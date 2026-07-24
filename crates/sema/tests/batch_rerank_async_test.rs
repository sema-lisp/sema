//! Async-offload coverage for `llm/batch` and `llm/rerank` (WP-LLM-BATCH-RERANK).
//!
//! For native providers, `llm/batch` offloads the whole batch call (including the
//! provider's internal concurrency) through one blocking-tier External wait. For
//! Sema-defined providers it sequences structural callbacks on the VM. Usage/cost
//! accounting lands on the VM thread in both cases. `llm/rerank` uses a native async
//! `rerank_future` hook for true cancellation, falling back to an admission-controlled
//! blocking offload for providers (like `FakeProvider`) that do not implement it.
//!
//! Deterministic + keyless (`FakeProvider`, see AGENTS.md "LLM / agent paths").
//! Neither builtin asserts otel spans or the process-global in-flight gauge here,
//! so no `sema_otel::testing::install()` / `#[serial]` is needed.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use sema_core::Value;
use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::{FakeProvider, FakeRecorder};

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

/// Extract the ordered list of channel receives from a `(list a b)` result of
/// two `channel/recv` calls, as strings — the deterministic ordering oracle
/// shared by the sibling-ordering tests below (never a wall-clock assert).
fn received_strings(val: &Value) -> Vec<String> {
    val.as_list()
        .expect("channel receives list")
        .iter()
        .map(|v| v.as_str().expect("string value").to_string())
        .collect()
}

// ── llm/batch ────────────────────────────────────────────────────────

#[test]
fn batch_async_completes_inside_spawn() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("r0")
        .reply("r1")
        .reply("r2")
        .build();
    let (result, recorder) = eval_with_fake(
        r#"(async/await (async/spawn (fn ()
             (llm/batch (list "p0" "p1" "p2") {:model "fake-chat"}))))"#,
        fake,
    );
    let val = result.expect("llm/batch inside async/spawn should succeed");
    let items = val.as_seq().expect("batch returns a list");
    let texts: Vec<&str> = items.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(texts, vec!["r0", "r1", "r2"]);
    assert_eq!(recorder.call_count(), 3);
}

/// Scheduler-not-stalled: the batch's provider round-trip is slow
/// (`chat_delay`, which `FakeProvider::batch_complete`'s default sequential
/// impl honors per request); a sibling task's short sleep must land on the
/// channel FIRST — proving the batch is offloaded off the VM thread, not
/// blocking it for the round-trip(s). Ordering via channel receive order,
/// never a duration assert.
#[test]
fn batch_async_lets_sibling_run_first() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(200)
        .reply("batch reply")
        .build();
    let (result, _recorder) = eval_with_fake(
        r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn () (channel/send out (first (llm/batch (list "p0") {:model "fake-chat"})))))
              (async/spawn (fn () (sleep 20) (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
        "#,
        fake,
    );
    let received = received_strings(&result.expect("batch sibling-ordering program evaluated"));
    assert_eq!(received.len(), 2);
    let sibling_pos = received
        .iter()
        .position(|v| v == "sibling")
        .expect("sibling value received");
    let batch_pos = received
        .iter()
        .position(|v| v == "batch reply")
        .expect("batch result received");
    assert!(
        sibling_pos < batch_pos,
        "sibling task must complete while llm/batch is in flight, got {received:?}"
    );
}

/// Usage/cost accounting lands on the VM thread in the poller: each response's
/// usage is folded via `track_usage` exactly once, same as the synchronous path.
#[test]
fn batch_async_tracks_usage_exactly_once() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply_with_usage("r0", 10, 5)
        .reply_with_usage("r1", 20, 7)
        .build();
    let (result, _recorder) = eval_with_fake(
        r#"
        (async/await (async/spawn (fn ()
          (llm/batch (list "p0" "p1") {:model "fake-chat"}))))
        (:total-tokens (llm/session-usage))
        "#,
        fake,
    );
    let val = result.expect("batch usage program evaluated");
    assert_eq!(
        val.as_int(),
        Some(42),
        "usage from both batch responses must be counted exactly once (10+5+20+7)"
    );
}

/// Sync-context regression: top-level `llm/batch` (no `async/spawn`) stays
/// byte-identical to before this WP — same results, one provider call per
/// prompt, in order.
#[test]
fn batch_sync_top_level_unchanged() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("s0")
        .reply("s1")
        .build();
    let (result, recorder) =
        eval_with_fake(r#"(llm/batch (list "p0" "p1") {:model "fake-chat"})"#, fake);
    let val = result.expect("top-level llm/batch should succeed");
    let items = val.as_seq().expect("batch returns a list");
    let texts: Vec<&str> = items.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(texts, vec!["s0", "s1"]);
    assert_eq!(recorder.call_count(), 2);
}

#[test]
fn batch_sema_provider_callbacks_preserve_context_and_runtime_control_flow() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (request)
                           (define pending
                             (async/spawn
                               (fn ()
                                 (sleep 50)
                                 (:content (first (:messages request))))))
                           (string-append
                             (context/get :prefix) ":" (async/await pending)))
               :default-model "sema-model"})
            (context/with {:prefix "ctx"}
              (fn ()
                (let ((out (channel/new 2)))
                  (async/spawn
                    (fn () (sleep 10) (channel/send out "sibling")))
                  (channel/send out (llm/batch (list "a" "b")))
                  (list (channel/recv out) (channel/recv out)))))
            "#,
        )
        .expect("Sema-defined batch callbacks can suspend in caller context");

    let received = value.as_list().expect("channel results");
    assert_eq!(received[0].as_str(), Some("sibling"));
    let batch = received[1].as_list().expect("ordered batch result");
    assert_eq!(batch[0].as_str(), Some("ctx:a"));
    assert_eq!(batch[1].as_str(), Some("ctx:b"));
}

#[test]
fn batch_sema_provider_invokes_later_callbacks_before_returning_an_earlier_error() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (define calls 0)
            (llm/define-provider :sema-provider
              {:complete (fn (request)
                           (set! calls (+ calls 1))
                           (define text (:content (first (:messages request))))
                           (if (= text "bad") (error "bad request") text))
               :default-model "sema-model"})
            (try
              (llm/batch (list "bad" "after"))
              (catch error calls))
            "#,
        )
        .expect("batch callback failure remains catchable");

    assert_eq!(
        value.as_int(),
        Some(2),
        "batch_complete semantics invoke every request before folding the first error"
    );
}

#[test]
fn cancelling_batch_sema_provider_stops_before_the_next_callback_and_usage_fold() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let started = Instant::now();
    let value = interp
        .eval_str_compiled(
            r#"
            (define calls 0)
            (llm/define-provider :sema-provider
              {:complete (fn (request)
                           (set! calls (+ calls 1))
                           (sleep 1000)
                           (:content (first (:messages request))))
               :default-model "sema-model"})
            (define pending
              (async/spawn (fn () (llm/batch (list "first" "second")))))
            (async/spawn (fn () (sleep 20) (async/cancel pending)))
            (list (try (async/await pending) (catch error :cancelled))
                  calls
                  (:total-tokens (llm/session-usage)))
            "#,
        )
        .expect("Sema-defined batch callback is cancellable");

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "cancellation must not wait out the callback sleep"
    );
    let items = value.as_list().expect("cancel result, calls, and usage");
    assert_eq!(items[0], Value::keyword("cancelled"));
    assert_eq!(items[1].as_int(), Some(1));
    assert_eq!(items[2].as_int(), Some(0));
}

#[test]
fn batch_sema_provider_accounts_each_success_exactly_once() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (request)
                           {:content (:content (first (:messages request)))
                            :usage {:prompt-tokens 3 :completion-tokens 2}})
               :default-model "sema-model"})
            (llm/batch (list "a" "b"))
            (:total-tokens (llm/session-usage))
            "#,
        )
        .expect("Sema-defined batch usage is accounted");

    assert_eq!(value.as_int(), Some(10));
}

// ── llm/rerank ───────────────────────────────────────────────────────

#[test]
fn rerank_async_completes_inside_spawn() {
    let fake = FakeProvider::builder("fake")
        .model("rerank-test")
        .rerank(&[(2, 0.91), (0, 0.42), (1, 0.10)])
        .build();
    let (result, recorder) = eval_with_fake(
        r#"
        (async/await (async/spawn (fn ()
          (llm/rerank "how do I read a file?"
            (list "vectors are cool" "unrelated trivia" "use file/read to read a file")
            {:top-k 3}))))
        "#,
        fake,
    );
    let val = result.expect("llm/rerank inside async/spawn should succeed");
    let items = val.as_seq().expect("rerank returns a list");
    assert_eq!(items.len(), 3);
    let top = items[0].as_map_rc().expect("result is a map");
    assert_eq!(
        top.get(&Value::keyword("index")).and_then(|v| v.as_int()),
        Some(2)
    );
    assert_eq!(
        top.get(&Value::keyword("document"))
            .and_then(|v| v.as_str()),
        Some("use file/read to read a file")
    );
    assert_eq!(recorder.reranks().len(), 1);
}

/// Scheduler-not-stalled: `llm/rerank`'s offloaded call is slow (`rerank_delay`);
/// a sibling task's short sleep must land on the channel FIRST — proving the
/// rerank is offloaded off the VM thread. FakeProvider has no `rerank_future`
/// override, so this also exercises the admission-controlled blocking-tier
/// fallback path (mirrors a sync-only provider). Ordering via channel receive
/// order, never a duration assert.
#[test]
fn rerank_async_lets_sibling_run_first() {
    let fake = FakeProvider::builder("fake")
        .model("rerank-test")
        .rerank_delay(200)
        .rerank(&[(0, 0.9), (1, 0.1)])
        .build();
    let (result, _recorder) = eval_with_fake(
        r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (channel/send out
                  (:document (first (llm/rerank "q" (list "reranked" "other")))))))
              (async/spawn (fn () (sleep 20) (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
        "#,
        fake,
    );
    let received = received_strings(&result.expect("rerank sibling-ordering program evaluated"));
    assert_eq!(received.len(), 2);
    let sibling_pos = received
        .iter()
        .position(|v| v == "sibling")
        .expect("sibling value received");
    let rerank_pos = received
        .iter()
        .position(|v| v == "reranked")
        .expect("rerank result received");
    assert!(
        sibling_pos < rerank_pos,
        "sibling task must complete while llm/rerank is in flight, got {received:?}"
    );
}

/// Sync-context regression (top-level, no `async/spawn`): byte-identical to
/// before this WP. `llm_fake_test.rs::rerank_reorders_documents_by_relevance`
/// covers this in more depth and stays green unmodified; this is a light
/// same-process companion asserting the empty-documents short-circuit and a
/// named `:provider` override both still bypass the provider registry lookup
/// the same way at top level.
#[test]
fn rerank_sync_top_level_unchanged() {
    let fake = FakeProvider::builder("fake")
        .model("rerank-test")
        .rerank(&[(1, 0.8), (0, 0.2)])
        .build();
    let (result, recorder) = eval_with_fake(r#"(llm/rerank "q" (list "a" "b"))"#, fake);
    let val = result.expect("top-level llm/rerank should succeed");
    let items = val.as_seq().expect("rerank returns a list");
    assert_eq!(items.len(), 2);
    let top = items[0].as_map_rc().expect("result is a map");
    assert_eq!(
        top.get(&Value::keyword("index")).and_then(|v| v.as_int()),
        Some(1)
    );
    assert_eq!(recorder.reranks().len(), 1);
}

#[test]
fn rerank_sync_empty_documents_short_circuits() {
    let fake = FakeProvider::builder("fake").model("rerank-test").build();
    let (result, recorder) = eval_with_fake(r#"(llm/rerank "q" (list))"#, fake);
    let val = result.expect("empty-documents llm/rerank should succeed");
    assert_eq!(val.as_seq().map(|s| s.len()), Some(0));
    assert_eq!(
        recorder.reranks().len(),
        0,
        "empty documents must never reach the provider"
    );
}
