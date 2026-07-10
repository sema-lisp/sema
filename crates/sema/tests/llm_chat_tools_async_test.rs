//! Async-offload coverage for `llm/chat`'s `:tools` branch (WP-LLM-CHAT-TOOLS).
//!
//! `llm/chat` with `:tools` used to run `run_tool_loop` — a Rust `for` over rounds
//! that calls the *synchronous* `do_complete` — unconditionally, even inside an
//! async scheduler task, freezing every sibling task for the whole multi-round
//! conversation. `llm/chat` is now a thin prelude dispatcher (mirrors `agent/run`):
//! in async context with a configured tool loop it drives the SAME
//! `__agent-step`/`__agent-exec-tools`/`__agent-finish`/`__agent-drive` machinery
//! that powers `agent/run` (via `__chat-begin`, built directly from the raw
//! messages + opts — llm/chat has no defagent/:session/:memory to unpack), so tool
//! rounds offload + yield and sibling tasks overlap. This closes the drift
//! documented in docs/plans/archive/2026-07-02-nonblocking-agent-run.md, whose plan
//! said both `agent/run` AND `llm/chat`-with-tools would become thin dispatchers —
//! only `agent/run` had shipped.
//!
//! Deterministic + keyless (`FakeProvider`, see AGENTS.md "LLM / agent paths").

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use sema_core::Value;
use sema_eval::Interpreter;
use sema_llm::builtins::{agent_runs_len, register_test_provider, reset_runtime_state};
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

const WEATHER_TOOL: &str = r#"
    (deftool get-weather
      "Get current weather for a city"
      {:city {:type :string :description "City name"}}
      (lambda (city) (str "sunny in " city)))
"#;

/// A multi-round tool loop started inside `async/spawn` completes with the correct
/// final reply, and the round-2 request carries the correlated tool-result turn —
/// the same oracle `agent_loop_round2_is_correlated` uses for `agent/run`. The
/// non-blocking slab (`AGENT_RUNS`, shared with `agent/run`) is empty afterward.
#[test]
fn tools_async_completes_multi_round_and_correlates_tool_result() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "get-weather", serde_json::json!({"city": "Oslo"}))
        .reply("It is sunny in Oslo.")
        .build();

    let src = format!(
        r#"
        {WEATHER_TOOL}
        (async/await (async/spawn (fn ()
          (llm/chat (list (message :user "What's the weather in Oslo?"))
            {{:model "fake-model" :tools [get-weather]}}))))
        "#
    );
    let (result, recorder) = eval_with_fake(&src, fake);
    let val = result.expect("llm/chat :tools inside async/spawn should succeed");
    assert_eq!(val.as_str(), Some("It is sunny in Oslo."));
    assert_eq!(
        recorder.call_count(),
        2,
        "expected exactly 2 provider rounds (tool call, then final answer)"
    );

    let reqs = recorder.requests();
    let round2 = &reqs[1];
    assert!(
        round2
            .messages
            .iter()
            .any(|m| m.role == "assistant" && !m.tool_calls.is_empty()),
        "round 2 must echo the assistant's tool_calls"
    );
    assert!(
        round2
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("call_1")),
        "round 2 must include a tool result correlated by tool_call_id"
    );
    assert_eq!(
        agent_runs_len(),
        0,
        "normal completion must leave the (shared agent/chat) slab empty"
    );
}

/// Scheduler-not-stalled: a sibling task's short sleep must land on the channel
/// BEFORE the tool loop's final reply — proving each round offloads + yields
/// instead of blocking the VM thread for the whole 2-round conversation. Ordering
/// via channel receive order, never a wall-clock assert.
#[test]
fn tools_async_lets_sibling_run_during_a_2round_loop() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(150)
        .tool_call("call_1", "get-weather", serde_json::json!({"city": "Oslo"}))
        .reply("It is sunny in Oslo.")
        .build();

    let src = format!(
        r#"
        {WEATHER_TOOL}
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (channel/send out (llm/chat (list (message :user "weather?"))
                                     {{:model "fake-model" :tools [get-weather]}}))))
              (async/spawn (fn () (sleep 20) (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
        "#
    );
    let (result, _recorder) = eval_with_fake(&src, fake);
    let val = result.expect("tools sibling-ordering program evaluated");
    let received: Vec<String> = val
        .as_list()
        .expect("channel receives list")
        .iter()
        .map(|v| v.as_str().expect("string value").to_string())
        .collect();
    assert_eq!(received.len(), 2);
    let sibling_pos = received
        .iter()
        .position(|v| v == "sibling")
        .expect("sibling value received");
    let chat_pos = received
        .iter()
        .position(|v| v == "It is sunny in Oslo.")
        .expect("chat result received");
    assert!(
        sibling_pos < chat_pos,
        "sibling task must complete while the tool loop's rounds are in flight, got {received:?}"
    );
}

/// `:max-tool-rounds` still caps the loop in async context, exactly like the
/// blocking `run_tool_loop`'s `for _round in 0..max_rounds` — the provider is
/// called exactly the cap's worth of times even though the script has more tool
/// rounds queued up, and the call does not error (mirrors
/// `agent_async_test::round_cap_with_pending_tools_leaves_valid_history`, which
/// covers the SAME shared `AgentLoopState`/`__agent-*` round-cap machinery this
/// native reuses).
#[test]
fn tools_async_enforces_max_tool_rounds() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_loop(3, "ping", serde_json::json!({"n": 1}), "never-reached")
        .build();
    let src = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (async/await (async/spawn (fn ()
          (llm/chat (list (message :user "go"))
            {:model "fake-model" :tools [ping] :max-tool-rounds 2}))))
    "#;
    let (result, recorder) = eval_with_fake(src, fake);
    result.expect("round-capped async tool loop must still complete, not error");
    assert_eq!(
        recorder.call_count(),
        2,
        "expected exactly max-tool-rounds (2) provider calls, got {}",
        recorder.call_count()
    );
    assert_eq!(
        agent_runs_len(),
        0,
        "round-capped completion empties the slab"
    );
}

/// `:tool-mode :none` in async context must NOT create a tool-loop handle at all —
/// it falls straight through to the plain (already async-aware) completion path,
/// so the provider never sees the tool schema and no tool call can happen.
#[test]
fn tools_async_tool_mode_none_skips_the_loop() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply("no tools used")
        .build();
    let src = format!(
        r#"
        {WEATHER_TOOL}
        (async/await (async/spawn (fn ()
          (llm/chat (list (message :user "hi"))
            {{:model "fake-model" :tools [get-weather] :tool-mode :none}}))))
        "#
    );
    let (result, recorder) = eval_with_fake(&src, fake);
    let val = result.expect("tool-mode :none inside async/spawn should succeed");
    assert_eq!(val.as_str(), Some("no tools used"));
    assert_eq!(recorder.call_count(), 1);
    assert_eq!(
        recorder.requests()[0].tools.len(),
        0,
        "tool-mode :none must not send tool schemas to the provider"
    );
    assert_eq!(agent_runs_len(), 0);
}

/// Cancelling an `llm/chat` `:tools` call mid-loop must not leak its slab entry —
/// the SAME task-reaped sweep that reclaims a cancelled `agent/run` reclaims it,
/// since both share the `AGENT_RUNS` slab (`reap_cancelled_agent_runs` matches by
/// owning task id, agnostic to agent-vs-chat). A subsequent call on the same
/// interpreter still completes normally. Mirrors
/// `agent_async_test::cancelled_agent_leaves_no_slab_entry_and_next_run_works`.
#[test]
fn tools_async_cancel_leaves_no_slab_entry_and_next_call_works() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(100)
        .tool_loop(8, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // 8 tool rounds (9 calls) at 100 ms each ~900 ms full; cancel at 250 ms.
    let cancel_src = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (let ((p (async/spawn (fn ()
                    (llm/chat (list (message :user "go"))
                      {:model "fake-model" :tools [ping] :max-tool-rounds 12})))))
          (try (async/timeout 250 p) (catch e nil)))
    "#;
    let _ = interp.eval_str_compiled(cancel_src);
    assert_eq!(
        agent_runs_len(),
        0,
        "cancelled llm/chat :tools slab entry must be reaped at the cancellation transition"
    );

    let next_src = r#"
        (async/await (async/spawn (fn ()
          (llm/chat (list (message :user "go"))
            {:model "fake-model" :tools [ping] :max-tool-rounds 12}))))
    "#;
    let val = interp
        .eval_str_compiled(next_src)
        .expect("llm/chat :tools after a cancelled call must still work");
    assert_eq!(val.as_str(), Some("done"));
    assert_eq!(
        agent_runs_len(),
        0,
        "normal completion after the cancelled call must leave the slab empty"
    );
}

/// Sync-context regression: `llm/chat` with `:tools` at top level is byte-identical
/// to before the split (`__llm-chat-blocking`'s `run_tool_loop` branch, unconverted).
#[test]
fn tools_sync_regression_outside_async() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "get-weather", serde_json::json!({"city": "Oslo"}))
        .reply("sync sunny in Oslo.")
        .build();
    let src = format!(
        r#"
        {WEATHER_TOOL}
        (llm/chat (list (message :user "weather?")) {{:model "fake-model" :tools [get-weather]}})
        "#
    );
    let (result, recorder) = eval_with_fake(&src, fake);
    let val = result.expect("llm/chat :tools at top level should succeed");
    assert_eq!(val.as_str(), Some("sync sunny in Oslo."));
    assert_eq!(recorder.call_count(), 2);
}

/// Capability gating survives the split into `__llm-chat-blocking` / `__chat-begin`:
/// a sandbox denying `Caps::LLM` must still reject `llm/chat` with `:tools` in async
/// context under the SAME `PermissionDenied { function: "llm/chat", .. }` a
/// sandboxed caller saw before this WP (`register_fn_ctx_gated_as` gates
/// `__chat-begin` under the public name, not its own registration name).
#[test]
fn tools_async_denied_by_sandbox_reports_llm_chat() {
    reset_runtime_state();
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::LLM);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let src = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (async/await (async/spawn (fn () (llm/chat (list (message :user "hi")) {:tools [ping]}))))
    "#;
    let err = interp
        .eval_str_compiled(src)
        .expect_err("llm/chat :tools must be denied when Caps::LLM is denied");
    let msg = err.to_string();
    assert!(
        msg.contains("llm/chat") && msg.contains("llm"),
        "expected a PermissionDenied naming llm/chat + the llm capability, got: {msg}"
    );
}
