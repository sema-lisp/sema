//! Acceptance gate for Task 04/06: the native `agent/run` tool loop must run its
//! tool-handler callbacks COOPERATIVELY when driven through the UNIFIED RUNTIME
//! (`eval_str_via_runtime`), so a tool handler that performs a runtime-suspending
//! op (here a synthetic `(await (async/spawn …))`, standing in for `mcp/call`'s
//! external wait) parks/resumes correctly and the multi-turn loop still completes.
//!
//! The correctness oracle is the LEGACY `eval_str` evaluator on the SAME program:
//! the runtime path must return byte-identical final answers. Includes the
//! tool-error-recovery contract (a tool that errors → fed back as a tool result →
//! the loop recovers), mirroring `mcp_builtin_test`.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use sema::{Interpreter, Value};
use sema_llm::builtins::{
    io_peak_inflight, register_test_provider, reset_io_inflight, reset_runtime_state,
};
use sema_llm::fake::{FakeProvider, FakeRecorder};
use serial_test::serial;

/// Eval `src` on a fresh interpreter through the LEGACY evaluator with `fake`
/// installed as the default provider — the correctness oracle.
fn oracle(src: &str, fake: FakeProvider) -> Result<Value, sema_core::SemaError> {
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));
    interp.eval_str(src)
}

/// Eval `src` on a fresh interpreter through the UNIFIED RUNTIME with `fake`
/// installed as the default provider — the path under test. Returns the result
/// and the recorder so tests can inspect the exact messages the loop built.
fn via_runtime(
    src: &str,
    fake: FakeProvider,
) -> (Result<Value, sema_core::SemaError>, Arc<FakeRecorder>) {
    let interp = Interpreter::new();
    reset_runtime_state();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    (interp.eval_str_via_runtime(src), recorder)
}

/// A tool whose handler SUSPENDS (spawns a task and awaits it) before returning
/// its argument — the synthetic stand-in for a tool that calls `mcp/call`.
const SUSPENDING_ECHO_AGENT: &str = r#"
    (deftool slow-echo "echo slowly" {:text {:type :string}}
      (fn (text) (await (async/spawn (fn () text)))))
    (defagent bot {:system "Use the tool." :model "fake-model"
                   :tools [slow-echo] :max-turns 5})
    (agent/run bot "echo hi")
"#;

fn suspending_echo_fake() -> FakeProvider {
    FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "slow-echo", serde_json::json!({ "text": "hi" }))
        .reply("all done")
        .build()
}

#[test]
#[serial]
fn blocking_agent_compatibility_native_rejects_runtime_entry() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "counted", serde_json::json!({"text": "hi"}))
        .reply("must not finish")
        .build();
    let (result, recorder) = via_runtime(
        r#"
        (define handler-calls 0)
        (deftool counted "Count calls" {:text {:type :string}}
          (fn (_text) (set! handler-calls (+ handler-calls 1))))
        (defagent bot {:model "fake-model" :tools [counted] :max-turns 3})
        (try
          (__agent-run-blocking bot "go")
          (catch error (list (:message error) handler-calls)))
        "#,
        fake,
    );

    let result = result
        .expect("runtime guard error should be catchable")
        .as_list()
        .expect("guard result list")
        .to_vec();
    let error = result[0].as_str().expect("guard error message");
    assert!(
        error.contains("__agent-run-blocking cannot run inside the cooperative runtime"),
        "unexpected blocking-native error: {error}"
    );
    assert_eq!(
        result[1],
        Value::int(0),
        "guard must run before tool handlers"
    );
    assert_eq!(
        recorder.call_count(),
        0,
        "guard must run before provider I/O"
    );
}

// GATE: a suspending tool handler inside `agent/run` completes the full
// multi-turn loop through the runtime and returns the final answer, matching the
// legacy `eval_str` oracle for the same program.
#[test]
#[serial]
fn agent_run_with_suspending_tool_handler_matches_oracle_via_runtime() {
    let expected = oracle(SUSPENDING_ECHO_AGENT, suspending_echo_fake())
        .expect("legacy oracle runs the agent loop");
    assert_eq!(expected.as_str(), Some("all done"), "oracle sanity");

    let (got, recorder) = via_runtime(SUSPENDING_ECHO_AGENT, suspending_echo_fake());
    let got = got.expect("runtime drives the suspending tool loop to completion");
    assert_eq!(
        got, expected,
        "runtime agent/run != legacy oracle for a suspending tool handler"
    );

    // Two provider rounds (tool round + reply), and round 2 must carry the value
    // the SUSPENDED handler returned — proof the handler ran cooperatively and its
    // result flowed back through the continuation into a correlated tool message.
    assert_eq!(
        recorder.call_count(),
        2,
        "expected a tool round then a reply"
    );
    let round2 = &recorder.requests()[1];
    let tool_msg = round2
        .messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("round 2 must include the correlated tool result");
    assert_eq!(
        tool_msg.content.as_text(),
        Some("hi"),
        "the suspending handler's return value must reach the model"
    );
}

/// A tool whose handler SUSPENDS and then ERRORS. The loop must feed the error
/// back to the model as a tool result and recover with the model's next reply.
const SUSPENDING_ERROR_AGENT: &str = r#"
    (deftool boom "always fails" {}
      (fn () (await (async/spawn (fn () (error "kaboom"))))))
    (defagent bot {:system "Try the tool." :model "fake-model"
                   :tools [boom] :max-turns 5})
    (agent/run bot "go")
"#;

fn suspending_error_fake() -> FakeProvider {
    FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "boom", serde_json::json!({}))
        .reply("recovered")
        .build()
}

// GATE: tool-error recovery through the runtime. A suspending handler that raises
// must not escape the loop; its error is fed back and the loop recovers, matching
// the legacy oracle.
#[test]
#[serial]
fn agent_run_tool_error_recovers_via_runtime() {
    let expected = oracle(SUSPENDING_ERROR_AGENT, suspending_error_fake())
        .expect("legacy oracle recovers from the tool error");
    assert_eq!(expected.as_str(), Some("recovered"), "oracle sanity");

    let (got, recorder) = via_runtime(SUSPENDING_ERROR_AGENT, suspending_error_fake());
    let got = got.expect("runtime recovers from a suspending tool error");
    assert_eq!(
        got, expected,
        "runtime tool-error recovery != legacy oracle"
    );

    // The handler's error must be fed back to the model as a tool result (not
    // escape the loop), so round 2 carries the error text.
    let round2 = &recorder.requests()[1];
    let tool_msg = round2
        .messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("round 2 must include the tool result");
    assert!(
        tool_msg
            .content
            .as_text()
            .unwrap_or_default()
            .contains("kaboom"),
        "the tool error detail should reach the model, got: {:?}",
        tool_msg.content.as_text()
    );
}

#[test]
#[serial]
fn agent_run_tool_schema_predicate_suspends_via_runtime() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call(
            "call_1",
            "validated-echo",
            serde_json::json!({ "text": "hi" }),
        )
        .reply("validated")
        .build();
    let program = r#"
        (deftool validated-echo "validated echo"
          {:text {:type :string
                  :validate (fn (text)
                    (await (async/spawn (fn () (= text "hi")))))} }
          (fn (text) text))
        (defagent bot {:system "Use the tool." :model "fake-model"
                       :tools [validated-echo] :max-turns 5})
        (agent/run bot "echo hi")
    "#;

    let (result, recorder) = via_runtime(program, fake);
    assert_eq!(
        result
            .expect("runtime drives the suspending schema predicate")
            .as_str(),
        Some("validated")
    );
    let round2 = &recorder.requests()[1];
    let tool_msg = round2
        .messages
        .iter()
        .find(|message| message.role == "tool")
        .expect("round 2 contains the correlated tool result");
    assert_eq!(tool_msg.content.as_text(), Some("hi"));
}

#[test]
#[serial]
fn cancelling_suspended_tool_schema_predicate_skips_handler() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call(
            "call_1",
            "validated-echo",
            serde_json::json!({ "text": "hi" }),
        )
        .build();
    let program = r#"
        (define predicate-entered (channel/new 1))
        (define handler-ran #f)
        (deftool validated-echo "validated echo"
          {:text {:type :string
                  :validate (fn (_text)
                    (channel/send predicate-entered #t)
                    (async/sleep 60000)
                    #t)} }
          (fn (text) (set! handler-ran #t) text))
        (defagent bot {:system "Use the tool." :model "fake-model"
                       :tools [validated-echo] :max-turns 5})
        (let ((run (async/spawn (fn () (agent/run bot "echo hi")))))
          (channel/recv predicate-entered)
          (async/cancel run)
          (try (async/await run) (catch error nil))
          handler-ran)
    "#;

    let (result, recorder) = via_runtime(program, fake);
    assert_eq!(
        result
            .expect("cancelling a schema predicate settles the agent task")
            .as_bool(),
        Some(false)
    );
    assert_eq!(recorder.call_count(), 1, "no later provider round runs");
}

#[test]
#[serial]
fn tool_schema_validation_preserves_observer_order_via_runtime() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call(
            "call_1",
            "validated-echo",
            serde_json::json!({ "text": "hi" }),
        )
        .reply("validated")
        .build();
    let program = r#"
        (define start-seen #f)
        (define handler-ran #f)
        (define events '())
        (deftool validated-echo "validated echo"
          {:text {:type :string :validate (fn (_text) start-seen)} }
          (fn (text) (set! handler-ran #t) text))
        (defagent bot {:system "Use the tool." :model "fake-model"
                       :tools [validated-echo] :max-turns 5})
        (define answer
          (:response
            (agent/run bot "echo hi"
              {:on-tool-call
               (fn (event)
                 (if (= (:event event) "start")
                     (begin
                       (set! start-seen #t)
                       (set! events (append events (list "start"))))
                     (set! events
                       (append events
                         (list (if (:error event) "end-error" "end-ok"))))))})))
        (list answer start-seen events handler-ran)
    "#;

    let (result, recorder) = via_runtime(program, fake);
    let result = result.expect("runtime agent completes");
    let fields = result.as_seq().expect("result tuple");
    assert_eq!(fields[0].as_str(), Some("validated"));
    assert_eq!(fields[1].as_bool(), Some(true));
    assert_eq!(fields[2].to_string(), r#"("start" "end-ok")"#);
    assert_eq!(fields[3].as_bool(), Some(true));
    let round2 = &recorder.requests()[1];
    let tool_msg = round2
        .messages
        .iter()
        .find(|message| message.role == "tool")
        .expect("round 2 contains the tool result");
    assert_eq!(tool_msg.content.as_text(), Some("hi"));
}

#[test]
#[serial]
fn failed_tool_schema_validation_emits_end_error_via_runtime() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call(
            "call_1",
            "validated-echo",
            serde_json::json!({ "text": "bad" }),
        )
        .reply("recovered")
        .build();
    let program = r#"
        (define handler-ran #f)
        (define events '())
        (deftool validated-echo "validated echo"
          {:text {:type :string :validate (fn (_text) #f) :message "rejected"} }
          (fn (text) (set! handler-ran #t) text))
        (defagent bot {:system "Use the tool." :model "fake-model"
                       :tools [validated-echo] :max-turns 5})
        (define answer
          (:response
            (agent/run bot "echo"
              {:on-tool-call
               (fn (event)
                 (set! events
                   (append events
                     (list
                       (if (= (:event event) "start")
                           "start"
                           (if (:error event) "end-error" "end-ok"))))))})))
        (list answer events handler-ran)
    "#;

    let (result, recorder) = via_runtime(program, fake);
    let result = result.expect("runtime agent recovers from schema rejection");
    let fields = result.as_seq().expect("result tuple");
    assert_eq!(fields[0].as_str(), Some("recovered"));
    assert_eq!(fields[1].to_string(), r#"("start" "end-error")"#);
    assert_eq!(fields[2].as_bool(), Some(false));
    let round2 = &recorder.requests()[1];
    let tool_msg = round2
        .messages
        .iter()
        .find(|message| message.role == "tool")
        .expect("round 2 contains the validation error");
    let content = tool_msg.content.as_text().unwrap_or_default();
    assert!(content.contains("invalid arguments"), "{content}");
    assert!(content.contains("rejected"), "{content}");
}

// GATE (full-flip blocker 1): two `agent/run`s spawned concurrently through the
// UNIFIED RUNTIME must OVERLAP across their provider rounds — not serialize. Each
// round now offloads the provider call to the executor IO pool and suspends the
// task on an External wait, so sibling agents run during every inter-round park.
// Proven two ways: peak offloaded futures in flight >= 2 AND a max-not-sum wall
// clock (3 agents × 3 rounds × 120 ms ⇒ serial floor ~1080 ms, overlapped ~360 ms).
#[test]
#[serial]
fn concurrent_agents_overlap_via_runtime() {
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(120)
        .tool_loop(2, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 6})
        (let ((t0 (sys/elapsed)))
          (async/all
            (map (fn (i) (async/spawn (fn () (agent/run bot "go"))))
                 (list 1 2 3)))
          (floor (/ (- (sys/elapsed) t0) 1000000)))
    "#;
    let wall = interp
        .eval_str_via_runtime(program)
        .expect("3 concurrent agents evaluated through the runtime");
    let wall_ms = wall.as_int().expect("wall ms");

    assert!(
        io_peak_inflight() >= 2,
        "expected peak offloaded futures in flight >= 2 (agents overlapping across \
         rounds), got {} — the runtime agent round still blocks the VM thread",
        io_peak_inflight()
    );
    assert!(
        wall_ms < 700,
        "expected overlapped wall < 700 ms (serial floor ~1080 ms), got {wall_ms} ms"
    );
}

// GATE (full-flip blocker 1): an `async/cancel` on a running `agent/run` driven
// through the UNIFIED RUNTIME must interrupt the loop promptly — no NEW provider
// round dispatches after the cutoff. Blocking today serialized all 9 rounds; the
// cooperative round parks between/inside rounds, so cancellation cuts it short.
#[test]
#[serial]
fn cancelling_agent_run_cuts_the_loop_short_via_runtime() {
    reset_io_inflight();

    // 8 tool rounds (9 calls) at 100 ms each ⇒ ~900 ms full; cancel at 250 ms.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(100)
        .tool_loop(8, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));

    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 12})
        (let ((p (async/spawn (fn () (agent/run bot "go")))))
          (async/spawn (fn () (async/sleep 250) (async/cancel p)))
          (try (async/await p) (catch e nil)))
    "#;
    let _ = interp.eval_str_via_runtime(program);

    let calls = recorder.call_count();
    assert!(
        calls > 0 && calls < 9,
        "expected the cancelled runtime agent to stop short of all 9 provider rounds, \
         but it made {calls} — cancellation could not interrupt the cooperative loop"
    );
}
