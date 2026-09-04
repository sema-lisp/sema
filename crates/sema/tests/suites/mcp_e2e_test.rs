use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use sema_core::testing::unique_temp_dir;

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
    // The built artifact is spawned below; Windows' CreateProcess resolves
    // extensionless program paths by appending ".exe", so name it that way.
    let out_path = tmp_dir.join(format!(
        "standalone_mcp_tool{}",
        std::env::consts::EXE_SUFFIX
    ));

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

/// Regression: tools that build a throwaway `Interpreter` or notebook `Engine`
/// inside the handler (`compile`, `disasm`, `notebook/new`) must not take the
/// server down when that value is dropped. Each interpreter owns LLM-provider
/// Tokio runtimes; dropping a plain runtime inside the server's own async
/// context panics ("Cannot drop a runtime in a context where blocking is not
/// allowed"). Under `panic = "abort"` (release) that aborts the whole process
/// mid-session; in debug the panic is caught but surfaces as an `isError` tool
/// result. `BlockingRuntime` makes the drop non-blocking.
///
/// This MUST run against the real `sema mcp` binary (not the in-process server):
/// the panic only fires under the binary's current-thread `block_on` runtime
/// context, which an in-process `#[tokio::test]` does not reproduce. We drive
/// all three tools plus a follow-up `eval` over one server lifetime and require
/// every call to succeed and the process to exit cleanly.
#[test]
fn test_mcp_e2e_interpreter_drop_does_not_crash_server() {
    let sema_bin = env!("CARGO_BIN_EXE_sema");
    let tmp_dir = unique_temp_dir("mcp-drop-e2e");
    let src_path = tmp_dir.join("sq.sema");
    let semac_path = tmp_dir.join("sq.semac");
    let nb_path = tmp_dir.join("nb.sema-nb");
    std::fs::write(&src_path, "(define (f x) (* x x))\n(f 7)\n").unwrap();

    let mut child = Command::new(sema_bin)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn sema mcp");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("Failed to open stdout"));

    let mut send = |req: serde_json::Value| {
        writeln!(stdin, "{}", req).unwrap();
        stdin.flush().unwrap();
    };
    fn read(stdout: &mut BufReader<std::process::ChildStdout>) -> serde_json::Value {
        let mut line = String::new();
        // A torn-down server returns 0 bytes here; surface that as a clear failure.
        let n = stdout.read_line(&mut line).unwrap();
        assert!(n > 0, "server closed the connection (likely crashed)");
        serde_json::from_str::<serde_json::Value>(&line).unwrap()
    }

    send(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }));
    let _ = read(&mut stdout);

    // Each of these three builds-and-drops an interpreter/engine in the handler.
    let calls = [
        (
            2,
            "compile",
            json!({ "source_path": src_path.to_str().unwrap(),
                    "output_path": semac_path.to_str().unwrap() }),
        ),
        (
            3,
            "disasm",
            json!({ "file_path": src_path.to_str().unwrap() }),
        ),
        (
            4,
            "notebook/new",
            json!({ "path": nb_path.to_str().unwrap(), "overwrite": true }),
        ),
    ];
    for (id, name, args) in calls {
        send(json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }));
        let resp = read(&mut stdout);
        assert_eq!(resp["id"], id);
        assert!(
            !resp["result"]["isError"].as_bool().unwrap_or(false),
            "tool '{name}' must not error/crash (drop-in-async-context regression): {resp}"
        );
    }

    // The server must still be alive and functional after all those drops.
    send(json!({
        "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": { "name": "eval", "arguments": { "code": "(* 6 7)" } }
    }));
    let resp = read(&mut stdout);
    assert_eq!(resp["id"], 5);
    assert!(
        resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("42"),
        "server must still evaluate after interpreter drops: {resp}"
    );

    // Clean shutdown: EOF on stdin -> loop exits 0 (no SIGABRT).
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(
        status.success() || status.code().is_none(),
        "server exited abnormally (code {:?}) — runtime-drop abort regression",
        status.code()
    );
    let _ = std::fs::remove_dir_all(&tmp_dir);
}
