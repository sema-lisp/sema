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
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
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
