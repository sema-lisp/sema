//! Deterministic real-tool-agent tests (slice S2).
//!
//! `(agent prompt {:tools [...]})` routes the leaf through the REAL `run_tool_loop` (via
//! `llm/chat`, which owns the tool dispatch) and journals each genuine tool call as an
//! `agent.tool_call` event through the `:on-tool-call` callback. Driven against a
//! scripted `FakeProvider` so the tool call is deterministic. Shared harness in
//! `workflow_common`.

mod workflow_common;
use workflow_common as wc;

use sema_llm::fake::FakeProvider;

const WEATHER_TOOL: &str = r#"
    (deftool get-weather
      "Get current weather for a city"
      {:city {:type :string :description "City name"}}
      (lambda (city) (str "{\"city\":\"" city "\",\"temp\":22}")))
"#;

#[test]
fn tools_agent_journals_one_real_tool_call() {
    // Round 1: the model emits a tool call. Round 2: a final reply. The real loop runs →
    // the on-tool-call callback fires once (gated on "start") → one agent.tool_call.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call(
            "call_1",
            "get-weather",
            serde_json::json!({ "city": "Oslo" }),
        )
        .reply("It is sunny in Oslo.")
        .build();

    let src = format!(
        r#"
        {WEATHER_TOOL}
        (defworkflow tool-demo
          "one tool-using agent"
          {{:phases ["Go"]}}
          (phase "Go")
          (def r (agent "What is the weather in Oslo?" {{:tools [get-weather] :name "scout"}}))
          {{:status :success :r r}})
    "#
    );

    let out = wc::run_once(&src, fake, "wf_tools_demo");

    // Exactly ONE agent.tool_call — the gate-on-"start" prevents the start+end double.
    let tool_calls = wc::events_of(&out.events, "agent.tool_call");
    assert_eq!(
        tool_calls.len(),
        1,
        "expected exactly one journaled tool call"
    );
    let tc = tool_calls[0];
    assert_eq!(tc["tool_name"], "get-weather");
    assert_eq!(tc["agent_id"], "scout_1", "attributes to the scout agent");
    let args_json = tc["args_json"].as_str().unwrap_or("");
    assert!(
        args_json.contains("Oslo") || args_json.contains("city"),
        "args_json should carry the real call args, got {args_json:?}"
    );

    assert_eq!(
        wc::events_of(&out.events, "run.ended")[0]["status"],
        "success"
    );
    let results = wc::events_of(&out.events, "agent.result");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["status"], "ok");
}

#[test]
fn tools_with_schema_returns_text_and_ignores_schema_v1() {
    // v1 contract: when BOTH :tools and :schema are present, the tools branch wins and
    // returns the loop's final TEXT — :schema does NOT compose yet. Pin this so the
    // deferral is guarded (flip to an explicit compose test when it lands).
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call(
            "call_1",
            "get-weather",
            serde_json::json!({ "city": "Oslo" }),
        )
        .reply("plain text answer, not json")
        .build();

    let src = format!(
        r#"
        {WEATHER_TOOL}
        (defworkflow tool-schema
          "tools + schema → text-only (v1)"
          {{:phases ["Go"]}}
          (phase "Go")
          (def r (agent "weather?" {{:tools [get-weather] :schema {{:temp :int}} :name "scout"}}))
          {{:status :success :r r}})
    "#
    );

    let out = wc::run_once(&src, fake, "wf_tools_schema");

    // The agent's output is the raw text (NOT parsed/validated against the schema).
    let result = wc::events_of(&out.events, "agent.result");
    assert_eq!(result[0]["status"], "ok");
    assert!(
        result[0]["output"]
            .as_str()
            .unwrap_or("")
            .contains("plain text"),
        "tools+schema must return loop text, not typed data: {:?}",
        result[0]["output"]
    );
    // The tool call still journaled.
    assert_eq!(wc::events_of(&out.events, "agent.tool_call").len(), 1);
}
