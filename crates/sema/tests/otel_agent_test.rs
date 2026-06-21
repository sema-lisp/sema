//! M2 acceptance: an `agent/run` with one tool produces the trace tree
//! `invoke_agent` → (`chat`, `execute_tool <name>`, `chat`) with correct
//! parent/child nesting and the tool's call.id. Deterministic (FakeProvider +
//! in-memory exporter). Own binary so the global provider / exporter is isolated.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;

#[test]
fn agent_run_emits_agent_tool_chat_tree() {
    let cap = sema_otel::testing::install();

    // Round 1: tool call. Round 2: final answer.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "get-weather", serde_json::json!({"city": "Oslo"}))
        .reply("It is sunny in Oslo.")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let src = r#"
        (deftool get-weather
          "Get current weather for a city"
          {:city {:type :string :description "City name"}}
          (lambda (city) (format "{\"city\": \"~a\", \"temp\": 22}" city)))
        (defagent weather-bot
          {:model "fake-model"
           :system "You are a weather assistant."
           :tools [get-weather]
           :max-turns 5})
        (agent/run weather-bot "What's the weather in Oslo?")
    "#;
    interp
        .eval_str_compiled(src)
        .expect("agent/run should complete against the fake");

    let spans = cap.spans_json();

    // The agent span (INTERNAL), named from the agent.
    let agent = spans
        .iter()
        .find(|s| s["attributes"]["gen_ai.operation.name"] == "invoke_agent")
        .expect("an invoke_agent span");
    assert_eq!(agent["kind"], "internal");
    assert_eq!(agent["name"], "invoke_agent weather-bot");
    let agent_id = agent["span_id"].as_str().unwrap();
    let trace_id = agent["trace_id"].as_str().unwrap();

    // The tool span (INTERNAL), named per v1.41, carrying tool.name + call.id + type.
    let tool = spans
        .iter()
        .find(|s| s["attributes"]["gen_ai.operation.name"] == "execute_tool")
        .expect("an execute_tool span");
    assert_eq!(tool["kind"], "internal");
    assert_eq!(tool["name"], "execute_tool get-weather");
    assert_eq!(tool["attributes"]["gen_ai.tool.name"], "get-weather");
    assert_eq!(tool["attributes"]["gen_ai.tool.call.id"], "call_1");
    assert_eq!(tool["attributes"]["gen_ai.tool.type"], "function");

    // Two chat spans (round 1 tool call, round 2 final answer).
    let chats: Vec<_> = spans
        .iter()
        .filter(|s| s["attributes"]["gen_ai.operation.name"] == "chat")
        .collect();
    assert_eq!(chats.len(), 2, "expected 2 chat spans, got {}", chats.len());

    // Nesting: tool + chat spans are children of the agent span, same trace.
    assert_eq!(tool["parent_span_id"], agent_id, "tool nests under agent");
    assert_eq!(tool["trace_id"], trace_id);
    for c in &chats {
        assert_eq!(c["parent_span_id"], agent_id, "chat nests under agent");
        assert_eq!(c["trace_id"], trace_id);
        assert_eq!(c["kind"], "client");
    }
}
