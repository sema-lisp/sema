use sema::{Interpreter, Value};

#[test]
fn test_mcp_builtins_can_connect_list_and_call() {
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

    let script_literal = serde_json::to_string(server_script).unwrap();
    let interp = Interpreter::new();

    interp
        .eval_str(&format!(
            r#"(define server (mcp/connect {{:command "python3" :args ["-c" {script_literal}]}}))"#
        ))
        .unwrap();

    let tools = interp.eval_str("(mcp/tools server)").unwrap();
    let tools_list = tools.as_seq().unwrap();
    assert_eq!(tools_list.len(), 1);
    let tool = tools_list[0].as_map_ref().unwrap();
    assert_eq!(
        tool.get(&Value::keyword("name")).unwrap().as_str().unwrap(),
        "echo"
    );

    let result = interp
        .eval_str(r#"(mcp/call server "echo" {:text "hello"})"#)
        .unwrap();
    let result_map = result.as_map_ref().unwrap();
    let content = result_map
        .get(&Value::keyword("content"))
        .unwrap()
        .as_seq()
        .unwrap();
    assert_eq!(content.len(), 1);
    let first = content[0].as_map_ref().unwrap();
    assert_eq!(
        first
            .get(&Value::keyword("text"))
            .unwrap()
            .as_str()
            .unwrap(),
        "hello"
    );

    interp.eval_str("(mcp/close server)").unwrap();
}

#[test]
fn test_mcp_tools_can_be_converted_into_agent_tool_defs() {
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

    let script_literal = serde_json::to_string(server_script).unwrap();
    let interp = Interpreter::new();

    interp
        .eval_str(&format!(
            r#"(define server (mcp/connect {{:command "python3" :args ["-c" {script_literal}]}}))"#
        ))
        .unwrap();

    let tool_defs = interp.eval_str("(mcp/->tools server)").unwrap();
    let defs = tool_defs.as_seq().unwrap();
    assert_eq!(defs.len(), 1);
    let tool_def = defs[0].as_tool_def_rc().unwrap();
    assert_eq!(tool_def.name, "echo");
    assert_eq!(tool_def.description, "Echo a string");

    let agent = interp
        .eval_str(r#"(defagent mcp-agent {:system "agent" :tools (mcp/->tools server) :model "test"})"#)
        .unwrap();
    let agent = agent.as_agent_rc().unwrap();
    assert_eq!(agent.tools.len(), 1);
    assert_eq!(agent.tools[0].as_tool_def_rc().unwrap().name, "echo");

    interp.eval_str("(mcp/close server)").unwrap();
}
