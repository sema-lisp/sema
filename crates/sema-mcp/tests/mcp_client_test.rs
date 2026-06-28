use sema_mcp::{McpClient, McpClientConfig};
use serde_json::json;

#[tokio::test]
async fn test_stdio_client_can_initialize_list_and_call_tools() {
    let server_script = r#"
import json
import sys

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue

    request = json.loads(line)
    method = request["method"]
    request_id = request["id"]

    if method == "initialize":
        response = {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {"name": "test-server", "version": "1.0"},
            },
        }
    elif method == "tools/list":
        response = {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo a string",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"text": {"type": "string"}},
                            "required": ["text"],
                        },
                    }
                ]
            },
        }
    elif method == "tools/call":
        args = request.get("params", {}).get("arguments", {})
        response = {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "content": [{"type": "text", "text": args.get("text", "")}],
                "isError": False,
            },
        }
    else:
        response = {
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32601, "message": "Method not found"},
        }

    sys.stdout.write(json.dumps(response) + "\n")
    sys.stdout.flush()
"#;

    let mut client = McpClient::connect({
        let mut config = McpClientConfig::new("python3");
        config.args = vec!["-c".to_string(), server_script.to_string()];
        config
    })
    .await
    .expect("failed to start MCP stdio client");

    let init_result = client
        .initialize()
        .await
        .expect("initialize request should succeed");
    assert_eq!(init_result["serverInfo"]["name"], "test-server");

    let tools = client
        .list_tools()
        .await
        .expect("tools/list request should succeed");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");

    let result = client
        .call_tool("echo", json!({ "text": "hello" }))
        .await
        .expect("tools/call request should succeed");
    assert!(!result.is_error);
    assert_eq!(result.content.len(), 1);
    assert!(matches!(
        &result.content[0],
        sema_mcp::protocol::ToolContent::Text { text } if text == "hello"
    ));

    client
        .close()
        .await
        .expect("closing the client should succeed");
}
