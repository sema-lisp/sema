//! Deterministic, key-free tests of the LLM/agent paths using a scripted
//! `FakeProvider`. This is the regression oracle for the agent tool loop —
//! including the round-2 tool-result message shape that the Phase 2 fix targets.
//!
//! No network, no API keys: `register_test_provider` installs the fake as the
//! default provider into the thread-local registry the runtime reads from.

use std::sync::Arc;

use sema_core::Value;
use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::{FakeProvider, FakeRecorder};

/// Build an interpreter, install `fake` as the default provider, run `src`.
/// Returns the eval result plus the recorder handle for asserting on the exact
/// requests the runtime built.
fn eval_with_fake(
    src: &str,
    fake: FakeProvider,
) -> (Result<Value, sema_core::SemaError>, Arc<FakeRecorder>) {
    let interp = Interpreter::new();
    // Fresh provider state, then install the fake as default.
    reset_runtime_state();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    let result = interp.eval_str_compiled(src);
    (result, recorder)
}

#[test]
fn llm_complete_returns_scripted_text() {
    let fake = FakeProvider::builder("fake").reply("hello there").build();
    let (result, recorder) = eval_with_fake(r#"(llm/complete "say hi")"#, fake);
    let val = result.expect("llm/complete should succeed against the fake");
    assert_eq!(val.as_str(), Some("hello there"));
    assert_eq!(recorder.call_count(), 1);
}

#[test]
fn agent_loop_completes_with_tool_call() {
    // Round 1: the model emits a tool call. Round 2 (after the tool result is fed
    // back): the model returns the final answer.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "get-weather", serde_json::json!({"city": "Oslo"}))
        .reply("It is sunny in Oslo.")
        .build();

    let src = r#"
        (deftool get-weather
          "Get current weather for a city"
          {:city {:type :string :description "City name"}}
          (lambda (city)
            (format "{\"city\": \"~a\", \"temp\": 22, \"condition\": \"sunny\"}" city)))

        (defagent weather-bot
          {:model "fake-model"
           :system "You are a weather assistant. Use tools. Be concise."
           :tools [get-weather]
           :max-turns 5})

        (agent/run weather-bot "What's the weather in Oslo?")
    "#;

    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("agent/run should complete against the fake");

    // The agent loop ran two provider rounds and returned the final answer.
    assert_eq!(val.as_str(), Some("It is sunny in Oslo."));
    assert_eq!(
        recorder.call_count(),
        2,
        "expected exactly 2 provider rounds (tool call, then final answer)"
    );

    // Round 2 must carry MORE messages than round 1 — i.e. the assistant turn and
    // the tool result were fed back into history. (The strict correlation check —
    // that the tool result is a correlated `role:tool` / tool_result message with
    // a tool_call_id — is asserted in `agent_loop_round2_is_correlated` once the
    // Phase 2 message model lands.)
    let reqs = recorder.requests();
    assert_eq!(reqs.len(), 2);
    assert!(
        reqs[1].messages.len() > reqs[0].messages.len(),
        "round 2 should include the fed-back tool result"
    );
}

/// Strict oracle for the Phase 2 tool-result protocol fix: round 2 must echo the
/// assistant's tool_calls and send the result as a correlated tool message
/// (`tool_call_id` matching the call). This is what OpenAI-family providers
/// require; before the fix the loop stuffed the result into plain user text with
/// no correlation, so the same agent looped to max-turns and returned empty.
#[test]
fn agent_loop_round2_is_correlated() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "get-weather", serde_json::json!({"city": "Oslo"}))
        .reply("It is sunny in Oslo.")
        .build();

    let src = r#"
        (deftool get-weather "Get weather"
          {:city {:type :string}}
          (lambda (city) "sunny"))
        (defagent weather-bot
          {:model "fake-model" :tools [get-weather] :max-turns 5})
        (agent/run weather-bot "weather in Oslo?")
    "#;

    let (result, recorder) = eval_with_fake(src, fake);
    result.expect("agent/run should complete");

    let reqs = recorder.requests();
    let round2 = &reqs[1];
    // An assistant message echoing the tool_calls...
    assert!(
        round2
            .messages
            .iter()
            .any(|m| m.role == "assistant" && !m.tool_calls.is_empty()),
        "round 2 must echo the assistant's tool_calls"
    );
    // ...followed by a correlated tool-result message keyed by the call id.
    assert!(
        round2
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("call_1")),
        "round 2 must include a tool result correlated by tool_call_id"
    );
}

// ── Phase 3: recoverable tool errors + argument validation ──────────────────

/// A handler that throws on round 1 must NOT abort the run; the error is fed back
/// and the model recovers on round 2.
#[test]
fn tool_handler_error_is_recoverable() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("c1", "flaky", serde_json::json!({"x": "bad"}))
        .reply("recovered")
        .build();
    let src = r#"
        (deftool flaky "A flaky tool" {:x {:type :string}}
          (lambda (x) (if (= x "bad") (throw "boom") "ok")))
        (defagent bot {:model "fake-model" :tools [flaky] :max-turns 5})
        (agent/run bot "use the tool")
    "#;
    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("agent/run must not abort on a tool error");
    assert_eq!(val.as_str(), Some("recovered"));
    assert_eq!(
        recorder.call_count(),
        2,
        "loop should continue after the tool error"
    );
}

/// A wrong-typed argument is rejected by schema validation (before the handler
/// runs), fed back, and the model retries successfully.
#[test]
fn tool_arg_validation_is_recoverable() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("c1", "calc", serde_json::json!({"n": "not-a-number"}))
        .reply("validated-ok")
        .build();
    let src = r#"
        (deftool calc "Needs a number" {:n {:type :number}}
          (lambda (n) (str n)))
        (defagent bot {:model "fake-model" :tools [calc] :max-turns 5})
        (agent/run bot "call calc")
    "#;
    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("agent/run must not abort on an arg validation error");
    assert_eq!(val.as_str(), Some("validated-ok"));
    assert_eq!(recorder.call_count(), 2);
}

/// Runaway error loops are bounded: 5 consecutive failing tool calls abort.
#[test]
fn consecutive_tool_errors_abort() {
    let mut b = FakeProvider::builder("fake").model("fake-model");
    for i in 0..6 {
        b = b.tool_call(&format!("c{i}"), "flaky", serde_json::json!({"x": "bad"}));
    }
    let fake = b.build();
    let src = r#"
        (deftool flaky "Always fails" {:x {:type :string}}
          (lambda (x) (throw "boom")))
        (defagent bot {:model "fake-model" :tools [flaky] :max-turns 10})
        (agent/run bot "go")
    "#;
    let (result, _recorder) = eval_with_fake(src, fake);
    let err = result.expect_err("runaway tool errors must abort");
    assert!(
        err.to_string().contains("consecutive tool errors"),
        "expected a consecutive-tool-errors abort, got: {err}"
    );
}

// ── Phase 4: network resilience (retry/backoff) ─────────────────────────────

use sema_llm::builtins::set_retry_base_ms;
use sema_llm::types::LlmError;

/// A transient 5xx is retried (broadened beyond 429) and the next attempt
/// succeeds. Backoff base is zeroed so the test asserts on attempt count, no sleep.
#[test]
fn transient_5xx_is_retried() {
    let fake = FakeProvider::builder("fake")
        .error(LlmError::Api {
            status: 503,
            message: "service unavailable".into(),
        })
        .reply("after-retry")
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    set_retry_base_ms(0); // no real sleeping
    register_test_provider(Box::new(fake));
    let val = interp
        .eval_str_compiled(r#"(llm/complete "hi")"#)
        .expect("should succeed after retrying the 5xx");
    assert_eq!(val.as_str(), Some("after-retry"));
    assert_eq!(recorder.call_count(), 2, "expected one retry after the 5xx");
}

/// A rate-limit (429) is retried too.
#[test]
fn rate_limit_is_retried() {
    let fake = FakeProvider::builder("fake")
        .error(LlmError::RateLimited { retry_after_ms: 1 })
        .reply("ok")
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    set_retry_base_ms(0);
    register_test_provider(Box::new(fake));
    let val = interp.eval_str_compiled(r#"(llm/complete "hi")"#).unwrap();
    assert_eq!(val.as_str(), Some("ok"));
    assert_eq!(recorder.call_count(), 2);
}

/// A non-retryable 4xx (e.g. 400) is NOT retried — it fails immediately.
#[test]
fn client_4xx_is_not_retried() {
    let fake = FakeProvider::builder("fake")
        .error(LlmError::Api {
            status: 400,
            message: "bad request".into(),
        })
        .reply("never-reached")
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    set_retry_base_ms(0);
    register_test_provider(Box::new(fake));
    let result = interp.eval_str_compiled(r#"(llm/complete "hi")"#);
    assert!(result.is_err(), "a 400 must not be retried");
    assert_eq!(recorder.call_count(), 1, "no retry on a 4xx");
}
