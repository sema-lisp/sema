use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("sema-{prefix}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

#[test]
fn test_mcp_e2e_initialize() {
    let sema_bin = env!("CARGO_BIN_EXE_sema");

    let mut child = Command::new(sema_bin)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn sema mcp");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("Failed to open stdout"));

    // 1. Send initialize request
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-e2e-client",
                "version": "1.0.0"
            }
        }
    });

    writeln!(stdin, "{}", init_req).unwrap();
    stdin.flush().unwrap();

    let mut resp_line = String::new();
    stdout.read_line(&mut resp_line).unwrap();

    let init_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
    assert_eq!(init_resp["jsonrpc"], "2.0");
    assert_eq!(init_resp["id"], 1);
    assert_eq!(init_resp["result"]["serverInfo"]["name"], "sema-mcp");

    // Clean up
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success() || status.code().is_none());
}

#[test]
fn test_mcp_e2e_filepath_mode() {
    let sema_bin = env!("CARGO_BIN_EXE_sema");
    let tmp_dir = unique_temp_dir("mcp-filepath-e2e");
    let file_path = tmp_dir.join("tools.sema");

    // Write a .sema file defining a custom tool
    let sema_code = r#"
(deftool my-mcp-add
  "E2E Test Tool"
  {:a {:type :number} :b {:type :number}}
  (lambda (a b) (+ a b)))
"#;
    std::fs::write(&file_path, sema_code).unwrap();

    let mut child = Command::new(sema_bin)
        .arg("mcp")
        .arg(file_path.to_str().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn sema mcp with filepath");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("Failed to open stdout"));

    // Send tools/list request
    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });

    writeln!(stdin, "{}", list_req).unwrap();
    stdin.flush().unwrap();

    let mut resp_line = String::new();
    stdout.read_line(&mut resp_line).unwrap();

    let list_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
    let tools = list_resp["result"]["tools"]
        .as_array()
        .expect("tools field must be an array");
    let tool_names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    assert!(
        tool_names.contains(&"my-mcp-add"),
        "Custom tool 'my-mcp-add' not found in: {:?}",
        tool_names
    );

    // Call the custom tool
    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "my-mcp-add",
            "arguments": {
                "a": 15,
                "b": 25
            }
        }
    });

    writeln!(stdin, "{}", call_req).unwrap();
    stdin.flush().unwrap();

    resp_line.clear();
    stdout.read_line(&mut resp_line).unwrap();

    let call_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
    assert_eq!(call_resp["id"], 2);
    let content = call_resp["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert_eq!(text, "40"); // (+ 15 25)

    // Clean up
    drop(stdin);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_mcp_e2e_standalone_binary_mode() {
    let sema_bin = env!("CARGO_BIN_EXE_sema");
    let tmp_dir = unique_temp_dir("mcp-standalone-e2e");
    let src_path = tmp_dir.join("tools.sema");
    let out_path = tmp_dir.join("standalone_mcp_tool");

    let sema_code = r#"
(deftool my-embedded-tool
  "Embedded custom tool"
  {:msg {:type :string}}
  (lambda (msg) (string-append "Got: " msg)))
"#;
    std::fs::write(&src_path, sema_code).unwrap();

    // 1. Build the standalone executable
    let build_output = Command::new(sema_bin)
        .args([
            "build",
            src_path.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to build standalone binary");

    assert!(
        build_output.status.success(),
        "sema build failed: {}",
        String::from_utf8_lossy(&build_output.stderr)
    );

    // 2. Spawn the standalone executable with --mcp flag
    let mut child = Command::new(out_path)
        .arg("--mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn standalone binary with --mcp");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("Failed to open stdout"));

    // Send tools/list request
    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });

    writeln!(stdin, "{}", list_req).unwrap();
    stdin.flush().unwrap();

    let mut resp_line = String::new();
    stdout.read_line(&mut resp_line).unwrap();

    let list_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
    let tools = list_resp["result"]["tools"]
        .as_array()
        .expect("tools list must be an array");
    let tool_names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    assert!(
        tool_names.contains(&"my-embedded-tool"),
        "Embedded custom tool not found in: {:?}",
        tool_names
    );

    // Call the embedded tool
    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "my-embedded-tool",
            "arguments": {
                "msg": "Hello E2E"
            }
        }
    });

    writeln!(stdin, "{}", call_req).unwrap();
    stdin.flush().unwrap();

    resp_line.clear();
    stdout.read_line(&mut resp_line).unwrap();

    let call_resp: serde_json::Value = serde_json::from_str(&resp_line).unwrap();
    assert_eq!(call_resp["id"], 2);
    let content = call_resp["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert_eq!(text, "Got: Hello E2E");

    // Clean up
    drop(stdin);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}
