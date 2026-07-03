//! Acceptance oracle for **non-blocking streaming** — `llm/stream` and agent
//! `:on-text` rounds inside async scheduler tasks.
//!
//! Previously a streaming round drove the provider's SSE stream via a blocking
//! call ON THE VM THREAD, freezing every sibling task for the stream's whole
//! duration (the sema-web dev-server head-of-line blocking). The fix applies the
//! ADR #68 "lift the loop to bytecode" pattern to streaming: the wire side runs
//! on the I/O pool sending deltas over a channel, and the prelude
//! `__stream-drive` loop parks on `AwaitIo` between delta batches, calling the
//! Sema callback per delta in TASK context.
//!
//! Deterministic + keyless: `FakeProvider` with `stream_chunk_delay` (a real
//! thread sleep between chunks on the wire side) is what gives a sibling ticker
//! wall time between deltas.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use sema_core::Value;
use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::{FakeProvider, FakeRecorder};
use sema_llm::types::LlmError;

/// Build an interpreter, install `fake` as the default provider, run `src`.
fn eval_with_fake(
    src: &str,
    fake: FakeProvider,
) -> (Result<Value, sema_core::SemaError>, Arc<FakeRecorder>) {
    let interp = Interpreter::new();
    reset_runtime_state();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    let result = interp.eval_str_compiled(src);
    (result, recorder)
}

/// THE oracle: a sibling ticker task advances DURING a streaming completion.
/// The streamer snapshots the tick count inside its on-chunk callback; with the
/// old inline stream the whole SSE drive ran atomically on the VM thread, so
/// every snapshot was equal (ticks accumulated between first and last delta =
/// 0). Non-blocking: the task parks between delta batches, the ticker runs, and
/// a later delta observes a strictly higher count.
#[test]
fn sibling_ticker_advances_during_llm_stream() {
    let chunks: Vec<String> = (0..10).map(|i| format!("c{i} ")).collect();
    let refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .stream(&refs)
        .stream_chunk_delay(30)
        .build();

    let program = r#"
        (define ticks (channel/new 4000))
        (define snaps '())
        (define (ticker)
          (dotimes (i 150)
            (async/sleep 3)
            (channel/send ticks 1)))
        (define (streamer)
          (llm/stream "go"
            (fn (c) (set! snaps (cons (channel/count ticks) snaps)))))
        (async/all (list (async/spawn (fn () (ticker)))
                         (async/spawn (fn () (streamer)))))
        ;; ticks accumulated between the FIRST and LAST delta
        (- (car snaps) (car (reverse snaps)))
    "#;
    let (result, recorder) = eval_with_fake(program, fake);
    let ticks_during = result
        .expect("ticker + streamer evaluated")
        .as_int()
        .expect("tick delta");
    assert!(
        ticks_during > 0,
        "expected the ticker to advance BETWEEN the stream's deltas (got {ticks_during} \
         ticks accumulated between first and last delta) — llm/stream froze the sibling task"
    );
    assert_eq!(recorder.call_count(), 1);
}

/// The agent counterpart: a `:on-text` streaming agent round must also let a
/// sibling ticker advance between its deltas.
#[test]
fn sibling_ticker_advances_during_agent_on_text_round() {
    let chunks: Vec<String> = (0..10).map(|i| format!("d{i} ")).collect();
    let refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .stream(&refs)
        .stream_chunk_delay(30)
        .build();

    let program = r#"
        (defagent bot {:model "fake-model"})
        (define ticks (channel/new 4000))
        (define snaps '())
        (define (ticker)
          (dotimes (i 150)
            (async/sleep 3)
            (channel/send ticks 1)))
        (define (run-agent)
          (agent/run bot "go"
            {:on-text (fn (c) (set! snaps (cons (channel/count ticks) snaps)))}))
        (async/all (list (async/spawn (fn () (ticker)))
                         (async/spawn (fn () (run-agent)))))
        (- (car snaps) (car (reverse snaps)))
    "#;
    let (result, recorder) = eval_with_fake(program, fake);
    let ticks_during = result
        .expect("ticker + agent evaluated")
        .as_int()
        .expect("tick delta");
    assert!(
        ticks_during > 0,
        "expected the ticker to advance BETWEEN the agent round's deltas (got \
         {ticks_during}) — the :on-text round froze the sibling task"
    );
    assert_eq!(recorder.call_count(), 1);
}

/// Ordering + exactly-once: in async context every delta reaches the callback
/// exactly once, in order, and the returned content equals their concatenation.
#[test]
fn async_stream_deltas_arrive_in_order_exactly_once() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .stream(&["Hel", "lo, ", "world"])
        .build();

    let program = r#"
        (define parts '())
        (define res
          (async/await (async/spawn (fn ()
            (llm/stream "p" (fn (c) (set! parts (cons c parts))))))))
        (list res (string/join (reverse parts) "") (length parts))
    "#;
    let (result, recorder) = eval_with_fake(program, fake);
    let val = result.expect("async llm/stream evaluated");
    let items = val.as_seq().expect("list result");
    assert_eq!(items[0].as_str(), Some("Hello, world"), "final content");
    assert_eq!(
        items[1].as_str(),
        Some("Hello, world"),
        "callback saw every delta, in order"
    );
    assert_eq!(items[2].as_int(), Some(3), "each chunk exactly once");
    assert_eq!(recorder.call_count(), 1);
}

/// Usage is accounted exactly once per streamed completion in async context
/// (the poller's finalize tracks; `__stream-finish` must not recharge).
#[test]
fn async_stream_accounts_usage_exactly_once() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .stream(&["a", "b", "c"])
        .build();

    // FakeProvider's stream response carries the default 10 prompt + 5
    // completion tokens; one streamed completion must add exactly 15.
    let program = r#"
        (define before (:total-tokens (llm/session-usage)))
        (async/await (async/spawn (fn () (llm/stream "p" (fn (c) nil)))))
        (- (:total-tokens (llm/session-usage)) before)
    "#;
    let (result, _) = eval_with_fake(program, fake);
    let delta = result
        .expect("async llm/stream evaluated")
        .as_int()
        .expect("token delta");
    assert_eq!(delta, 15, "one streamed completion charged exactly once");
}

/// A YIELDING callback inside the stream loop now works: the callback runs in
/// task context, so `async/sleep` inside it parks legally (previously the yield
/// leaked mid-native — undefined behavior, documented as unsupported).
#[test]
fn async_stream_callback_may_itself_yield() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .stream(&["one ", "two ", "three"])
        .build();

    let program = r#"
        (define out "")
        (async/await (async/spawn (fn ()
          (llm/stream "p"
            (fn (c)
              (async/sleep 1)
              (set! out (string-append out c)))))))
        out
    "#;
    let (result, _) = eval_with_fake(program, fake);
    let out = result.expect("yielding callback stream evaluated");
    assert_eq!(
        out.as_str(),
        Some("one two three"),
        "a callback that yields (async/sleep) still sees every delta"
    );
}

/// The agent `:on-text` yielding-callback counterpart: a callback that parks
/// (async/sleep) during a streaming agent round completes the round correctly.
#[test]
fn agent_on_text_callback_may_itself_yield() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .stream(&["ha", "iku"])
        .build();

    let program = r#"
        (defagent bot {:model "fake-model"})
        (define trace "")
        (define r
          (async/await (async/spawn (fn ()
            (agent/run bot "hi"
              {:on-text (fn (c)
                          (async/sleep 1)
                          (set! trace (string-append trace c "|")))})))))
        (list (:response r) trace)
    "#;
    let (result, recorder) = eval_with_fake(program, fake);
    let items = result.expect("agent with yielding :on-text evaluated");
    let items = items.as_seq().expect("list result");
    assert_eq!(items[0].as_str(), Some("haiku"), "final response");
    assert_eq!(items[1].as_str(), Some("ha|iku|"), "deltas in order");
    assert_eq!(recorder.call_count(), 1);
}

/// Async agent streaming through a tool round: round 1 issues a tool call,
/// round 2 streams the final answer — mirroring the sync
/// `agent_run_on_text_streams_after_a_tool_round` oracle, now on the
/// non-blocking path. Tool-result correlation feeds
/// `agent_apply_step_response` unchanged.
#[test]
fn async_agent_on_text_streams_after_a_tool_round() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "calc", serde_json::json!({"x": 2}))
        .stream(&["The ", "answer ", "is 4"])
        .build();

    let program = r#"
        (deftool calc "double a number" {:x {:type :number :description "n"}}
          (lambda (x) "4"))
        (defagent bot {:model "fake-model" :tools [calc] :max-turns 5})
        (define trace "")
        (define r
          (async/await (async/spawn (fn ()
            (agent/run bot "double 2"
              {:on-text (fn (c)
                          (when (> (string/length c) 0)
                            (set! trace (string-append trace c "|"))))})))))
        (list (:response r) trace)
    "#;
    let (result, recorder) = eval_with_fake(program, fake);
    let val = result.expect("async streaming agent through a tool round evaluated");
    let items = val.as_seq().expect("list result");
    assert_eq!(items[0].as_str(), Some("The answer is 4"), "final response");
    assert_eq!(
        items[1].as_str(),
        Some("The |answer |is 4|"),
        "only round 2's text streams, in order"
    );
    assert_eq!(
        recorder.call_count(),
        2,
        "two provider calls: tool round + streamed reply"
    );
    // Round-2 correlation survived the streaming round: the second request must
    // carry the tool result for call_1.
    let round2 = &recorder.requests()[1];
    assert!(
        round2
            .messages
            .iter()
            .any(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("call_1")),
        "round-2 request must carry the correlated tool result"
    );
}

/// Mid-stream failure in async context keeps the sync path's ordering contract:
/// every delta delivered before the failure reaches the callback FIRST, then
/// the error surfaces as a catchable error (no auto-retry, no re-emit).
#[test]
fn async_stream_mid_stream_error_after_partial_deltas() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .stream_then_error(
            &["par", "tial"],
            LlmError::Http("connection dropped mid-stream".to_string()),
        )
        .build();

    let program = r#"
        (define got "")
        (define err nil)
        (async/await (async/spawn (fn ()
          (try (llm/stream "p" (fn (c) (set! got (string-append got c))))
               (catch e (set! err (format "~a" e)))))))
        (list got err)
    "#;
    let (result, recorder) = eval_with_fake(program, fake);
    let val = result.expect("mid-stream failure evaluated");
    let items = val.as_seq().expect("list result");
    assert_eq!(
        items[0].as_str(),
        Some("partial"),
        "partial deltas reach the callback before the error"
    );
    let err = items[1].as_str().unwrap_or_default().to_string();
    assert!(
        err.contains("dropped mid-stream"),
        "mid-stream error surfaces (got: {err})"
    );
    assert_eq!(
        recorder.call_count(),
        1,
        "no auto-retry on mid-stream failure"
    );
}

/// Cancelling a task parked mid-stream (via `async/timeout`) abandons the run
/// (best-effort: the wire worker streams to completion into a dead channel,
/// discarded) and leaves the runtime healthy — a fresh stream completes.
#[test]
fn cancelled_stream_is_cut_short_and_runtime_stays_healthy() {
    // 40 chunks x 50 ms ≈ 2 s stream against a 150 ms timeout.
    let chunks: Vec<String> = (0..40).map(|i| format!("s{i}")).collect();
    let refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .stream(&refs)
        .stream_chunk_delay(50)
        .stream(&["ok"])
        .build();

    let program = r#"
        (define got 0)
        (try (async/timeout 150
               (async/spawn (fn ()
                 (llm/stream "slow" (fn (c) (set! got (+ got 1)))))))
             (catch e nil))
        (define out "")
        (async/await (async/spawn (fn ()
          (llm/stream "again" (fn (c) (set! out (string-append out c)))))))
        (list got out)
    "#;
    let (result, _) = eval_with_fake(program, fake);
    let val = result.expect("cancelled stream + follow-up evaluated");
    let items = val.as_seq().expect("list result");
    let got = items[0].as_int().unwrap_or(-1);
    assert!(
        got < 40,
        "timeout must cut the stream short (saw {got} of 40 deltas)"
    );
    assert_eq!(
        items[1].as_str(),
        Some("ok"),
        "a fresh stream after the cancellation completes normally"
    );
}
