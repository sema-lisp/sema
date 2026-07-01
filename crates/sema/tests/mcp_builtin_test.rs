//! Sema-facing MCP client builtin tests (`mcp/connect|tools|call|tools->sema`),
//! including the agent adapter driven end-to-end through the real agent loop
//! against a scripted `FakeProvider` (no network, no keys) and a live stdio MCP
//! server (a small Python script).

use std::sync::Arc;

use sema::{Interpreter, InterpreterBuilder, Value};
use sema_core::{Caps, Sandbox};
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::{FakeProvider, FakeRecorder};

/// A stdio MCP server exposing `echo` (echoes its `text` argument) and `boom`
/// (always returns an MCP error). Refuses `tools/list` until it has seen the
/// `notifications/initialized` message, matching a conformant server.
const SERVER: &str = r#"
import json
import sys

initialized = False

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    request = json.loads(line)
    method = request.get("method")
    request_id = request.get("id")

    if request_id is None:
        if method == "notifications/initialized":
            initialized = True
        continue

    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "serverInfo": {"name": "test-server", "version": "1.0"}}})
    elif method == "tools/list":
        if not initialized:
            send({"jsonrpc": "2.0", "id": request_id,
                  "error": {"code": -32002, "message": "not initialized"}})
        else:
            send({"jsonrpc": "2.0", "id": request_id, "result": {"tools": [
                {"name": "echo", "description": "Echo a string",
                 "inputSchema": {"type": "object",
                                 "properties": {"text": {"type": "string"}},
                                 "required": ["text"]}},
                {"name": "boom", "description": "Always fails",
                 "inputSchema": {"type": "object", "properties": {}}},
            ]}})
    elif method == "tools/call":
        name = request.get("params", {}).get("name")
        args = request.get("params", {}).get("arguments", {})
        if name == "boom":
            send({"jsonrpc": "2.0", "id": request_id, "result": {
                "content": [{"type": "text", "text": "kaboom"}], "isError": True}})
        else:
            send({"jsonrpc": "2.0", "id": request_id, "result": {
                "content": [{"type": "text", "text": args.get("text", "")}],
                "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": request_id,
              "error": {"code": -32601, "message": "Method not found"}})
"#;

/// Sema that connects to the server above and binds the handle to `server`.
fn connect_expr() -> String {
    let encoded = serde_json::to_string(SERVER).unwrap();
    format!(r#"(define server (mcp/connect {{:command "python3" :args ["-c" {encoded}]}}))"#)
}

#[test]
fn test_mcp_builtins_connect_list_call() {
    let interp = Interpreter::new();
    interp.eval_str(&connect_expr()).unwrap();

    let tools = interp.eval_str("(mcp/tools server)").unwrap();
    let tools = tools.as_seq().unwrap();
    assert_eq!(tools.len(), 2);
    let first = tools[0].as_map_ref().unwrap();
    assert_eq!(
        first.get(&Value::keyword("name")).unwrap().as_str(),
        Some("echo")
    );

    // Layer 1 `mcp/call` collapses text content to a string.
    let result = interp
        .eval_str(r#"(mcp/call server "echo" {:text "hello"})"#)
        .unwrap();
    assert_eq!(result.as_str(), Some("hello"));

    interp.eval_str("(mcp/close server)").unwrap();
}

#[test]
fn test_mcp_tools_to_sema_uses_deftool_param_shape() {
    let interp = Interpreter::new();
    interp.eval_str(&connect_expr()).unwrap();

    let defs = interp.eval_str("(mcp/tools->sema server)").unwrap();
    let defs = defs.as_seq().unwrap();
    let echo = defs
        .iter()
        .find_map(|d| d.as_tool_def_rc().filter(|td| td.name == "echo"))
        .expect("echo tool def");
    assert_eq!(echo.description, "Echo a string");

    // The parameters must be a `{param-name -> spec}` map (like `deftool`), NOT
    // the raw JSON Schema — otherwise the agent loop maps model arguments over
    // `:type`/`:properties`/`:required` and the tool is called with nil args.
    let params = echo.parameters.as_map_ref().expect("params map");
    assert!(
        params.contains_key(&Value::keyword("text")),
        "expected a :text parameter, got keys: {:?}",
        params.keys().collect::<Vec<_>>()
    );
    assert!(!params.contains_key(&Value::keyword("properties")));
    assert!(!params.contains_key(&Value::keyword("type")));

    interp.eval_str("(mcp/close server)").unwrap();
}

/// Build a `sema::Interpreter`, install `fake` as the default provider, eval `src`.
fn eval_with_fake(
    interp: &Interpreter,
    src: &str,
    fake: FakeProvider,
) -> (Result<Value, sema_core::SemaError>, Arc<FakeRecorder>) {
    reset_runtime_state();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    (interp.eval_str(src), recorder)
}

#[test]
fn test_mcp_agent_tool_call_round_trips_arguments() {
    // The model calls `echo` with {text: "ping"}; the server echoes it back; the
    // model then produces a final answer. Proves the whole adapter -> agent loop
    // -> mcp/call -> server path passes the argument through correctly.
    let interp = Interpreter::new();
    interp.eval_str(&connect_expr()).unwrap();
    interp
        .eval_str(
            r#"(defagent mcp-agent
                 {:system "Use the echo tool." :model "fake-model"
                  :tools (mcp/tools->sema server) :max-turns 5})"#,
        )
        .unwrap();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "echo", serde_json::json!({ "text": "ping" }))
        .reply("all done")
        .build();

    let (result, recorder) = eval_with_fake(&interp, r#"(agent/run mcp-agent "echo ping")"#, fake);
    assert_eq!(result.unwrap().as_str(), Some("all done"));
    assert_eq!(
        recorder.call_count(),
        2,
        "expected a tool round then a reply"
    );

    // The round-2 request must carry the tool result the server produced — proof
    // the "ping" argument actually reached the server (a nil-arg bug echoes "").
    let round2 = &recorder.requests()[1];
    let tool_msg = round2
        .messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("round 2 must include the correlated tool result");
    assert_eq!(tool_msg.content.as_text(), Some("ping"));

    interp.eval_str("(mcp/close server)").unwrap();
}

#[test]
fn test_mcp_tool_error_surfaces_to_agent() {
    // A tool that returns `isError: true` must surface as an error the loop feeds
    // back to the model, not a silent success.
    let interp = Interpreter::new();
    interp.eval_str(&connect_expr()).unwrap();
    interp
        .eval_str(
            r#"(defagent boom-agent
                 {:system "Try the tool." :model "fake-model"
                  :tools (mcp/tools->sema server) :max-turns 5})"#,
        )
        .unwrap();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "boom", serde_json::json!({}))
        .reply("recovered")
        .build();

    let (result, recorder) = eval_with_fake(&interp, r#"(agent/run boom-agent "go")"#, fake);
    assert_eq!(result.unwrap().as_str(), Some("recovered"));

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

    interp.eval_str("(mcp/close server)").unwrap();
}

#[test]
fn test_mcp_connect_denied_without_process_capability() {
    // Connecting spawns a process, so a sandbox that denies PROCESS must refuse.
    let interp = InterpreterBuilder::new()
        .with_sandbox(Sandbox::deny(Caps::PROCESS))
        .build();
    let err = interp
        .eval_str(r#"(mcp/connect {:command "python3" :args ["-c" "pass"]})"#)
        .expect_err("mcp/connect must be denied without the process capability");
    let msg = err.to_string();
    assert!(
        msg.contains("process") || msg.contains("capability") || msg.contains("mcp/connect"),
        "unexpected error: {msg}"
    );
}

#[test]
fn mcp_builtins_validate_arguments() {
    let interp = Interpreter::new();
    // Arity.
    assert!(interp.eval_str("(mcp/call)").is_err());
    assert!(interp.eval_str(r#"(mcp/call "h")"#).is_err());
    assert!(interp.eval_str("(mcp/tools)").is_err());
    // Handle must be a string.
    assert!(interp.eval_str(r#"(mcp/call 42 "tool" {})"#).is_err());
    assert!(interp.eval_str("(mcp/tools 42)").is_err());
    // Unknown/closed handle is a clean error, not a panic.
    let err = interp
        .eval_str(r#"(mcp/call "mcp-does-not-exist" "tool" {})"#)
        .expect_err("unknown handle must error");
    assert!(
        err.to_string().contains("not registered"),
        "unexpected error: {err}"
    );
    // Non-string tool name.
    let conn = connect_expr();
    interp.eval_str(&conn).expect("connect");
    assert!(interp.eval_str("(mcp/call server 42 {})").is_err());
    interp.eval_str("(mcp/close server)").ok();
}
