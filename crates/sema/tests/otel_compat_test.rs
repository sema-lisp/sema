//! SEMA_OTEL_COMPAT: with the compat layer active, every backend's native alias keys
//! are emitted alongside the canonical gen_ai.* attrs. Deterministic (FakeProvider +
//! in-memory exporter). Own binary (compat override + global provider are process-global).

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;

#[test]
fn compat_all_emits_backend_aliases() {
    unsafe {
        std::env::set_var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "true");
    }
    sema_otel::testing::set_compat("all");
    let cap = sema_otel::testing::install();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("c1", "get-x", serde_json::json!({"q": 1}))
        .reply("final answer")
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let src = r#"
        (deftool get-x "desc" {:q {:type :number}} (lambda (q) "42"))
        (defagent bot {:model "fake-model" :tools [get-x]})
        (agent/run bot "go")
    "#;
    interp.eval_str_compiled(src).expect("agent run");

    let spans = cap.spans_json();
    let attr = |op: &str, key: &str| -> serde_json::Value {
        spans
            .iter()
            .find(|s| s["attributes"]["gen_ai.operation.name"] == op)
            .map(|s| s["attributes"][key].clone())
            .unwrap_or(serde_json::Value::Null)
    };

    // chat (LLM) span — every backend's kind + model + tokens + identity.
    assert_eq!(attr("chat", "openinference.span.kind"), "LLM");
    assert_eq!(attr("chat", "llm.model_name"), "fake-model");
    assert_eq!(attr("chat", "llm.provider"), "fake");
    assert_eq!(attr("chat", "llm.token_count.prompt"), 10);
    assert_eq!(attr("chat", "llm.token_count.completion"), 5);
    assert_eq!(attr("chat", "traceloop.span.kind"), "task");
    assert_eq!(attr("chat", "llm.request.type"), "chat");
    assert_eq!(attr("chat", "llm.usage.total_tokens"), 15);
    assert_eq!(attr("chat", "gen_ai.usage.prompt_tokens"), 10);
    assert_eq!(attr("chat", "langsmith.span.kind"), "llm");
    assert_eq!(attr("chat", "gen_ai.system"), "fake");
    assert_eq!(attr("chat", "langfuse.observation.type"), "generation");
    assert_eq!(
        attr("chat", "langfuse.observation.model.name"),
        "fake-model"
    );
    // content I/O aliases (capture on)
    assert!(
        attr("chat", "input.value").is_string(),
        "openinference input.value"
    );
    assert_eq!(attr("chat", "input.mime_type"), "application/json");
    assert!(attr("chat", "traceloop.entity.input").is_string());
    // usage_details JSON parses
    let ud = attr("chat", "langfuse.observation.usage_details");
    let ud: serde_json::Value = serde_json::from_str(ud.as_str().unwrap()).unwrap();
    assert_eq!(ud["total"], 15);

    // tool span kinds
    assert_eq!(attr("execute_tool", "openinference.span.kind"), "TOOL");
    assert_eq!(attr("execute_tool", "traceloop.span.kind"), "tool");
    assert_eq!(attr("execute_tool", "langsmith.span.kind"), "tool");

    // agent span kinds
    assert_eq!(attr("invoke_agent", "openinference.span.kind"), "AGENT");
    assert_eq!(attr("invoke_agent", "traceloop.span.kind"), "agent");
    assert_eq!(attr("invoke_agent", "langsmith.span.kind"), "chain");
    assert_eq!(attr("invoke_agent", "langfuse.observation.type"), "span");

    // Advertised tool schemas on the chat span (OpenInference + Traceloop).
    assert!(
        attr("chat", "llm.tools.0.tool.json_schema").is_string(),
        "OpenInference advertised tool schema"
    );
    assert_eq!(attr("chat", "llm.request.functions.0.name"), "get-x");

    // Tool args + result on the execute_tool span (content-gated).
    assert!(attr("execute_tool", "gen_ai.tool.call.arguments").is_string());
    assert!(attr("execute_tool", "tool_call.function.arguments").is_string());
    assert_eq!(attr("execute_tool", "output.value"), "42");
    assert_eq!(attr("execute_tool", "traceloop.entity.output"), "42");

    // Trace-level I/O rollup on the agent root (Langfuse trace panel).
    assert!(attr("invoke_agent", "langfuse.trace.input").is_string());
    assert!(attr("invoke_agent", "langfuse.trace.output").is_string());
}
