//! Stdio MCP client tests. The scripted Python server deliberately (a) refuses
//! `tools/list` until it has received `notifications/initialized`, and (b)
//! interleaves an id-less notification ahead of each response — so a client that
//! skips the initialized notification or blindly reads one line per request
//! fails here.

use sema_mcp::{McpClient, McpClientConfig};
use serde_json::json;

/// A minimal, correct-ish MCP stdio server used across the tests below.
const ECHO_SERVER: &str = r#"
import json
import sys

initialized = False

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

def note(method):
    # An id-less JSON-RPC notification; a correct client must skip it while
    # waiting for its correlated response.
    send({"jsonrpc": "2.0", "method": method, "params": {}})

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    request = json.loads(line)
    method = request.get("method")
    request_id = request.get("id")

    # Notifications carry no id and expect no response.
    if request_id is None:
        if method == "notifications/initialized":
            initialized = True
        continue

    if method == "initialize":
        note("notifications/message")  # interleaved chatter before the reply
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "serverInfo": {"name": "test-server", "version": "1.0"},
        }})
    elif method == "tools/list":
        if not initialized:
            send({"jsonrpc": "2.0", "id": request_id,
                  "error": {"code": -32002, "message": "not initialized"}})
        else:
            note("notifications/progress")
            send({"jsonrpc": "2.0", "id": request_id, "result": {"tools": [
                {"name": "echo", "description": "Echo a string",
                 "inputSchema": {"type": "object",
                                 "properties": {"text": {"type": "string"}},
                                 "required": ["text"]}},
            ]}})
    elif method == "tools/call":
        args = request.get("params", {}).get("arguments", {})
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "content": [{"type": "text", "text": args.get("text", "")}],
            "isError": False,
        }})
    else:
        send({"jsonrpc": "2.0", "id": request_id,
              "error": {"code": -32601, "message": "Method not found"}})
"#;

fn echo_config() -> McpClientConfig {
    let mut config = McpClientConfig::new("python3");
    config.args = vec!["-c".to_string(), ECHO_SERVER.to_string()];
    config
}

#[tokio::test]
async fn test_stdio_client_initializes_lists_and_calls() {
    let mut client = McpClient::connect(echo_config())
        .await
        .expect("failed to start MCP stdio client");

    let init = client
        .initialize()
        .await
        .expect("initialize request should succeed");
    assert_eq!(init["serverInfo"]["name"], "test-server");

    // Succeeds only because initialize() also sent notifications/initialized and
    // the client skipped the interleaved notifications while correlating.
    let tools = client
        .list_tools()
        .await
        .expect("tools/list should succeed after initialized notification");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");

    let result = client
        .call_tool("echo", json!({ "text": "hello" }))
        .await
        .expect("tools/call should succeed");
    assert_eq!(result["content"][0]["text"], "hello");
    assert_eq!(result["isError"], false);

    client.close().await.expect("closing should succeed");
}

#[tokio::test]
async fn test_stdio_client_reports_missing_command() {
    let err =
        match McpClient::connect(McpClientConfig::new("definitely-not-a-real-binary-xyz")).await {
            Ok(_) => panic!("connecting to a missing binary should fail"),
            Err(err) => err,
        };
    assert!(
        err.contains("failed to spawn MCP server process"),
        "unexpected error: {err}"
    );
}
