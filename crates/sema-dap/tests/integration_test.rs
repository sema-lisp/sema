use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

fn sema_binary() -> String {
    // Find the sema binary in the target directory
    let mut path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    path.push("sema");
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path.to_string_lossy().to_string()
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("sema-dap-{prefix}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

fn send_dap(stdin: &mut impl Write, seq: u64, command: &str, args: Option<serde_json::Value>) {
    let mut msg = serde_json::json!({
        "seq": seq,
        "type": "request",
        "command": command,
    });
    if let Some(a) = args {
        msg.as_object_mut()
            .unwrap()
            .insert("arguments".to_string(), a);
    }
    let body = serde_json::to_string(&msg).unwrap();
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    stdin.write_all(header.as_bytes()).unwrap();
    stdin.write_all(body.as_bytes()).unwrap();
    stdin.flush().unwrap();
}

/// Read a DAP message from the child process stdout with a timeout.
fn read_dap_timeout(
    reader: &mut BufReader<impl Read>,
    timeout: Duration,
) -> Option<serde_json::Value> {
    let deadline = std::time::Instant::now() + timeout;

    let mut header = String::new();
    let mut content_length: Option<usize> = None;
    loop {
        if std::time::Instant::now() > deadline {
            return None;
        }
        header.clear();
        let n = reader.read_line(&mut header).ok()?;
        if n == 0 {
            return None;
        }
        let trimmed = header.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(len_str) = trimmed.strip_prefix("Content-Length: ") {
            content_length = len_str.parse().ok();
        }
    }
    let len = content_length?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// Default read timeout for DAP messages (10 seconds).
const DAP_TIMEOUT: Duration = Duration::from_secs(10);

fn read_dap(reader: &mut BufReader<impl Read>) -> Option<serde_json::Value> {
    read_dap_timeout(reader, DAP_TIMEOUT)
}

/// Wait for a specific DAP event, skipping unrelated messages.
fn wait_for_event(
    reader: &mut BufReader<impl Read>,
    event_name: &str,
    max_messages: usize,
) -> bool {
    for _ in 0..max_messages {
        if let Some(msg) = read_dap(reader) {
            if msg["type"] == "event" && msg["event"] == event_name {
                return true;
            }
        } else {
            return false;
        }
    }
    false
}

fn wait_for_event_message(
    reader: &mut BufReader<impl Read>,
    event_name: &str,
    max_messages: usize,
) -> Option<serde_json::Value> {
    for _ in 0..max_messages {
        let msg = read_dap(reader)?;
        if msg["type"] == "event" && msg["event"] == event_name {
            return Some(msg);
        }
    }
    None
}

#[test]
fn test_dap_initialize_and_disconnect() {
    let binary = sema_binary();

    let mut child = Command::new(&binary)
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Send initialize
    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));

    // Read initialize response
    let resp = read_dap(&mut reader).expect("should get initialize response");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["command"], "initialize");
    assert_eq!(resp["success"], true);
    assert_eq!(
        resp["body"]["supportsConfigurationDoneRequest"], true,
        "should support configurationDone"
    );

    // Read initialized event
    let event = read_dap(&mut reader).expect("should get initialized event");
    assert_eq!(event["type"], "event");
    assert_eq!(event["event"], "initialized");

    // Send disconnect
    send_dap(&mut stdin, 2, "disconnect", None);

    // Read disconnect response
    let resp = read_dap(&mut reader).expect("should get disconnect response");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["command"], "disconnect");
    assert_eq!(resp["success"], true);

    // Process should exit
    let status = child.wait().expect("failed to wait for child");
    assert!(
        status.success(),
        "sema dap should exit cleanly after disconnect"
    );
}

#[test]
fn test_dap_launch_and_run() {
    let binary = sema_binary();

    // Create a simple test program in a unique temp dir
    let dir = unique_temp_dir("launch");
    let program_path = dir.join("test.sema");
    std::fs::write(&program_path, "(+ 1 2)\n").unwrap();

    let mut child = Command::new(&binary)
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Initialize
    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap(); // response
    let _event = read_dap(&mut reader).unwrap(); // initialized event

    // Launch
    send_dap(
        &mut stdin,
        2,
        "launch",
        Some(serde_json::json!({
            "program": program_path.to_string_lossy(),
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "launch");
    assert_eq!(resp["success"], true);

    // ConfigurationDone — this triggers execution
    send_dap(&mut stdin, 3, "configurationDone", None);
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "configurationDone");
    assert_eq!(resp["success"], true);

    // Should get terminated event (program runs to completion)
    // May get output events first — allow up to 50 messages
    assert!(
        wait_for_event(&mut reader, "terminated", 50),
        "should receive terminated event"
    );

    // Disconnect
    send_dap(&mut stdin, 4, "disconnect", None);
    let _ = read_dap(&mut reader); // disconnect response

    let status = child.wait().expect("failed to wait for child");
    assert!(status.success());

    // Cleanup
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_breakpoint_and_continue() {
    let binary = sema_binary();

    let dir = unique_temp_dir("bp");
    let program_path = dir.join("test_bp.sema");
    // Multi-line program — set breakpoint on line 2
    std::fs::write(&program_path, "(define x 1)\n(define y 2)\n(+ x y)\n").unwrap();

    let mut child = Command::new(&binary)
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Initialize
    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    let _event = read_dap(&mut reader).unwrap();

    // Set breakpoint on line 2 before launch
    send_dap(
        &mut stdin,
        2,
        "setBreakpoints",
        Some(serde_json::json!({
            "source": { "path": program_path.to_string_lossy() },
            "breakpoints": [{ "line": 3 }],
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "setBreakpoints");
    assert_eq!(resp["success"], true);

    // Launch with stopOnEntry = false
    send_dap(
        &mut stdin,
        3,
        "launch",
        Some(serde_json::json!({
            "program": program_path.to_string_lossy(),
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "launch");
    assert_eq!(resp["success"], true);

    // ConfigurationDone
    send_dap(&mut stdin, 4, "configurationDone", None);
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "configurationDone");
    assert_eq!(resp["success"], true);

    // Should get a stopped event (breakpoint or step) — allow up to 50 messages
    assert!(
        wait_for_event(&mut reader, "stopped", 50),
        "should receive stopped event at breakpoint"
    );

    // Request stack trace while stopped
    send_dap(&mut stdin, 5, "stackTrace", Some(serde_json::json!({})));
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "stackTrace");
    assert_eq!(resp["success"], true);
    let frames = resp["body"]["stackFrames"].as_array().unwrap();
    assert!(!frames.is_empty(), "should have at least one stack frame");

    // Continue execution
    send_dap(&mut stdin, 6, "continue", Some(serde_json::json!({})));
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "continue");
    assert_eq!(resp["success"], true);

    // Should get terminated event — allow up to 50 messages
    assert!(
        wait_for_event(&mut reader, "terminated", 50),
        "should receive terminated event"
    );

    // Disconnect
    send_dap(&mut stdin, 7, "disconnect", None);
    let _ = read_dap(&mut reader);

    let status = child.wait().expect("failed to wait for child");
    assert!(status.success());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_breakpoint_after_launch() {
    let binary = sema_binary();

    let dir = unique_temp_dir("bp_after");
    let program_path = dir.join("test_bp.sema");
    std::fs::write(&program_path, "(define x 1)\n(define y 2)\n(+ x y)\n").unwrap();

    let mut child = Command::new(&binary)
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Initialize
    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    let _event = read_dap(&mut reader).unwrap();

    // Launch first
    send_dap(
        &mut stdin,
        2,
        "launch",
        Some(serde_json::json!({
            "program": program_path.to_string_lossy(),
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "launch");
    assert_eq!(resp["success"], true);

    // Now set breakpoint (after launch, before configurationDone)
    send_dap(
        &mut stdin,
        3,
        "setBreakpoints",
        Some(serde_json::json!({
            "source": { "path": program_path.to_string_lossy() },
            "breakpoints": [{ "line": 3 }],
        })),
    );
    // If it deadlocks, this read will timeout (return None)
    let resp = read_dap_timeout(&mut reader, Duration::from_secs(2))
        .expect("Deadlock detected: setBreakpoints request blocked indefinitely!");
    assert_eq!(resp["command"], "setBreakpoints");
    assert_eq!(resp["success"], true);

    // ConfigurationDone
    send_dap(&mut stdin, 4, "configurationDone", None);
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "configurationDone");
    assert_eq!(resp["success"], true);

    // Should get a stopped event
    assert!(
        wait_for_event(&mut reader, "stopped", 50),
        "should receive stopped event at breakpoint"
    );

    // Continue
    send_dap(&mut stdin, 5, "continue", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();

    // Terminated
    assert!(
        wait_for_event(&mut reader, "terminated", 50),
        "should receive terminated event"
    );

    // Disconnect
    send_dap(&mut stdin, 6, "disconnect", None);
    let _ = read_dap(&mut reader);

    let status = child.wait().expect("failed to wait for child");
    assert!(status.success());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_breakpoint_on_blank_line_slides_to_executable_line() {
    let binary = sema_binary();

    let dir = unique_temp_dir("bp_slide");
    let program_path = dir.join("test_bp_slide.sema");
    std::fs::write(&program_path, "(define x 1)\n\n; comment\n(+ x 2)\n").unwrap();

    let mut child = Command::new(&binary)
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    let _event = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        2,
        "launch",
        Some(serde_json::json!({
            "program": program_path.to_string_lossy(),
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "launch");
    assert_eq!(resp["success"], true);

    send_dap(
        &mut stdin,
        3,
        "setBreakpoints",
        Some(serde_json::json!({
            "source": { "path": program_path.to_string_lossy() },
            "breakpoints": [{ "line": 3 }],
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "setBreakpoints");
    assert_eq!(resp["success"], true);
    let bp = &resp["body"]["breakpoints"][0];
    assert_eq!(bp["verified"], true);
    assert_eq!(bp["line"], 4);
    assert!(
        bp["message"]
            .as_str()
            .unwrap_or_default()
            .contains("line 4"),
        "slid breakpoint should explain resolved line: {bp}"
    );

    send_dap(&mut stdin, 4, "configurationDone", None);
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "configurationDone");
    assert_eq!(resp["success"], true);

    let event = wait_for_event_message(&mut reader, "stopped", 50)
        .expect("should receive stopped event at slid breakpoint");
    assert_eq!(event["body"]["reason"], "breakpoint");

    send_dap(&mut stdin, 5, "continue", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    assert!(wait_for_event(&mut reader, "terminated", 50));

    send_dap(&mut stdin, 6, "disconnect", None);
    let _ = read_dap(&mut reader);
    let status = child.wait().expect("failed to wait for child");
    assert!(status.success());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_evaluate_and_set_variable_while_stopped() {
    let binary = sema_binary();

    let dir = unique_temp_dir("eval_set");
    let program_path = dir.join("test_eval_set.sema");
    std::fs::write(
        &program_path,
        "(define global 5)\n(define (f x)\n  (+ x global))\n(f 10)\n",
    )
    .unwrap();

    let mut child = Command::new(&binary)
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    let init = read_dap(&mut reader).unwrap();
    assert_eq!(init["body"]["supportsEvaluateForHovers"], true);
    assert_eq!(init["body"]["supportsSetVariable"], true);
    let _event = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        2,
        "setBreakpoints",
        Some(serde_json::json!({
            "source": { "path": program_path.to_string_lossy() },
            "breakpoints": [{ "line": 3 }],
        })),
    );
    let _resp = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        3,
        "launch",
        Some(serde_json::json!({
            "program": program_path.to_string_lossy(),
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "launch");
    assert_eq!(resp["success"], true);

    send_dap(&mut stdin, 4, "configurationDone", None);
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "configurationDone");
    assert_eq!(resp["success"], true);
    wait_for_event_message(&mut reader, "stopped", 50).expect("should stop at breakpoint");

    send_dap(&mut stdin, 5, "stackTrace", Some(serde_json::json!({})));
    let resp = read_dap(&mut reader).unwrap();
    let frame_id = resp["body"]["stackFrames"]
        .as_array()
        .unwrap()
        .iter()
        .find(|frame| frame["name"] == "f")
        .and_then(|frame| frame["id"].as_u64())
        .expect("stack trace should include f frame");

    send_dap(
        &mut stdin,
        6,
        "evaluate",
        Some(serde_json::json!({
            "expression": "(+ x global)",
            "frameId": frame_id,
            "context": "watch",
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "evaluate");
    assert_eq!(resp["success"], true);
    assert_eq!(resp["body"]["result"], "15");
    assert_eq!(resp["body"]["type"], "int");

    send_dap(
        &mut stdin,
        7,
        "scopes",
        Some(serde_json::json!({ "frameId": frame_id })),
    );
    let resp = read_dap(&mut reader).unwrap();
    let locals_ref = resp["body"]["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scope| scope["name"] == "Locals")
        .and_then(|scope| scope["variablesReference"].as_u64())
        .expect("locals scope should exist");

    send_dap(
        &mut stdin,
        8,
        "setVariable",
        Some(serde_json::json!({
            "variablesReference": locals_ref,
            "name": "x",
            "value": "32",
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "setVariable");
    assert_eq!(resp["success"], true);
    assert_eq!(resp["body"]["value"], "32");
    assert_eq!(resp["body"]["type"], "int");

    send_dap(
        &mut stdin,
        9,
        "evaluate",
        Some(serde_json::json!({
            "expression": "(+ x global)",
            "frameId": frame_id,
            "context": "watch",
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);
    assert_eq!(resp["body"]["result"], "37");

    send_dap(&mut stdin, 10, "continue", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    assert!(wait_for_event(&mut reader, "terminated", 50));

    send_dap(&mut stdin, 11, "disconnect", None);
    let _ = read_dap(&mut reader);
    let status = child.wait().expect("failed to wait for child");
    assert!(status.success());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_evaluate_before_execution_returns_error() {
    let binary = sema_binary();

    let mut child = Command::new(&binary)
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    let _event = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        2,
        "evaluate",
        Some(serde_json::json!({
            "expression": "x",
            "frameId": 0,
        })),
    );
    let resp = read_dap_timeout(&mut reader, Duration::from_secs(2))
        .expect("evaluate before execution should return immediately");
    assert_eq!(resp["command"], "evaluate");
    assert_eq!(resp["success"], false);

    send_dap(&mut stdin, 3, "disconnect", None);
    let _ = read_dap(&mut reader);
    let status = child.wait().expect("failed to wait for child");
    assert!(status.success());
}

#[test]
fn test_dap_named_upvalue_evaluate_and_set_variable_while_stopped() {
    let binary = sema_binary();

    let dir = unique_temp_dir("named_upvalue");
    let program_path = dir.join("test_named_upvalue.sema");
    std::fs::write(
        &program_path,
        "(define (make-adder base)\n  (lambda (x)\n    (+ base x)))\n(define add5 (make-adder 5))\n(add5 10)\n",
    )
    .unwrap();

    let mut child = Command::new(&binary)
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    let _event = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        2,
        "setBreakpoints",
        Some(serde_json::json!({
            "source": { "path": program_path.to_string_lossy() },
            "breakpoints": [{ "line": 3 }],
        })),
    );
    let _resp = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        3,
        "launch",
        Some(serde_json::json!({
            "program": program_path.to_string_lossy(),
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);

    send_dap(&mut stdin, 4, "configurationDone", None);
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);
    wait_for_event_message(&mut reader, "stopped", 50).expect("should stop at breakpoint");

    send_dap(&mut stdin, 5, "stackTrace", Some(serde_json::json!({})));
    let resp = read_dap(&mut reader).unwrap();
    let frame_id = resp["body"]["stackFrames"][0]["id"].as_u64().unwrap();

    send_dap(
        &mut stdin,
        6,
        "scopes",
        Some(serde_json::json!({ "frameId": frame_id })),
    );
    let resp = read_dap(&mut reader).unwrap();
    let closure_ref = resp["body"]["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scope| scope["name"] == "Closure")
        .and_then(|scope| scope["variablesReference"].as_u64())
        .expect("closure scope should exist");

    send_dap(
        &mut stdin,
        7,
        "variables",
        Some(serde_json::json!({ "variablesReference": closure_ref })),
    );
    let resp = read_dap(&mut reader).unwrap();
    let upvalues = resp["body"]["variables"].as_array().unwrap();
    assert!(
        upvalues
            .iter()
            .any(|var| var["name"] == "base" && var["value"] == "5"),
        "Closure scope should expose lexical upvalue name: {upvalues:?}"
    );

    send_dap(
        &mut stdin,
        8,
        "evaluate",
        Some(serde_json::json!({
            "expression": "(+ base x)",
            "frameId": frame_id,
            "context": "watch",
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);
    assert_eq!(resp["body"]["result"], "15");

    send_dap(
        &mut stdin,
        9,
        "setVariable",
        Some(serde_json::json!({
            "variablesReference": closure_ref,
            "name": "base",
            "value": "20",
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);
    assert_eq!(resp["body"]["value"], "20");

    send_dap(
        &mut stdin,
        10,
        "evaluate",
        Some(serde_json::json!({
            "expression": "(+ base x)",
            "frameId": frame_id,
            "context": "watch",
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);
    assert_eq!(resp["body"]["result"], "30");

    send_dap(&mut stdin, 11, "continue", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    assert!(wait_for_event(&mut reader, "terminated", 50));

    send_dap(&mut stdin, 12, "disconnect", None);
    let _ = read_dap(&mut reader);
    let status = child.wait().expect("failed to wait for child");
    assert!(status.success());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_variables_expand_evaluate_result_lazily() {
    let binary = sema_binary();

    let dir = unique_temp_dir("compound_expand");
    let program_path = dir.join("test_compound_expand.sema");
    std::fs::write(
        &program_path,
        "(define (f xs)\n  (list xs))\n(f (list 1 (list 2 3)))\n",
    )
    .unwrap();

    let mut child = Command::new(&binary)
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    let _event = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        2,
        "setBreakpoints",
        Some(serde_json::json!({
            "source": { "path": program_path.to_string_lossy() },
            "breakpoints": [{ "line": 2 }],
        })),
    );
    let _resp = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        3,
        "launch",
        Some(serde_json::json!({
            "program": program_path.to_string_lossy(),
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);

    send_dap(&mut stdin, 4, "configurationDone", None);
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);
    wait_for_event_message(&mut reader, "stopped", 50).expect("should stop at breakpoint");

    send_dap(&mut stdin, 5, "stackTrace", Some(serde_json::json!({})));
    let resp = read_dap(&mut reader).unwrap();
    let frame_id = resp["body"]["stackFrames"][0]["id"].as_u64().unwrap();

    send_dap(
        &mut stdin,
        6,
        "evaluate",
        Some(serde_json::json!({
            "expression": "xs",
            "frameId": frame_id,
            "context": "watch",
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);
    let xs_ref = resp["body"]["variablesReference"]
        .as_u64()
        .expect("evaluate result should include variablesReference");
    assert!(
        xs_ref > 0,
        "compound evaluate result should be expandable: {resp}"
    );

    send_dap(
        &mut stdin,
        7,
        "variables",
        Some(serde_json::json!({ "variablesReference": xs_ref })),
    );
    let resp = read_dap(&mut reader).unwrap();
    let children = resp["body"]["variables"].as_array().unwrap();
    assert_eq!(children.len(), 2);
    assert_eq!(children[0]["name"], "[0]");
    assert_eq!(children[0]["value"], "1");
    assert_eq!(children[1]["name"], "[1]");
    let nested_ref = children[1]["variablesReference"].as_u64().unwrap();
    assert!(nested_ref > 0);

    send_dap(
        &mut stdin,
        8,
        "variables",
        Some(serde_json::json!({ "variablesReference": nested_ref })),
    );
    let resp = read_dap(&mut reader).unwrap();
    let nested = resp["body"]["variables"].as_array().unwrap();
    assert_eq!(nested.len(), 2);
    assert_eq!(nested[0]["value"], "2");
    assert_eq!(nested[1]["value"], "3");

    send_dap(&mut stdin, 9, "continue", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    assert!(wait_for_event(&mut reader, "terminated", 50));

    send_dap(&mut stdin, 10, "disconnect", None);
    let _ = read_dap(&mut reader);
    let status = child.wait().expect("failed to wait for child");
    assert!(status.success());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_variables_expand_record_fields_by_name() {
    let binary = sema_binary();

    let dir = unique_temp_dir("record_expand");
    let program_path = dir.join("test_record_expand.sema");
    std::fs::write(
        &program_path,
        "\
(define-record-type point (make-point x y) point? (x point-x) (y point-y))
(define (f p)
  p)
(f (make-point 3 4))
",
    )
    .unwrap();

    let mut child = Command::new(&binary)
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    let _event = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        2,
        "setBreakpoints",
        Some(serde_json::json!({
            "source": { "path": program_path.to_string_lossy() },
            "breakpoints": [{ "line": 3 }],
        })),
    );
    let _resp = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        3,
        "launch",
        Some(serde_json::json!({
            "program": program_path.to_string_lossy(),
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);

    send_dap(&mut stdin, 4, "configurationDone", None);
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);
    wait_for_event_message(&mut reader, "stopped", 50).expect("should stop at breakpoint");

    send_dap(&mut stdin, 5, "stackTrace", Some(serde_json::json!({})));
    let resp = read_dap(&mut reader).unwrap();
    let frame_id = resp["body"]["stackFrames"][0]["id"].as_u64().unwrap();

    send_dap(
        &mut stdin,
        6,
        "evaluate",
        Some(serde_json::json!({
            "expression": "(make-point 3 4)",
            "frameId": frame_id,
            "context": "watch",
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);
    let point_ref = resp["body"]["variablesReference"]
        .as_u64()
        .expect("record evaluate result should include variablesReference");
    assert!(
        point_ref > 0,
        "record evaluate result should be expandable: {resp}"
    );

    send_dap(
        &mut stdin,
        7,
        "variables",
        Some(serde_json::json!({ "variablesReference": point_ref })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);
    let fields = resp["body"]["variables"].as_array().unwrap();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0]["name"], "x");
    assert_eq!(fields[0]["value"], "3");
    assert_eq!(fields[1]["name"], "y");
    assert_eq!(fields[1]["value"], "4");

    send_dap(&mut stdin, 8, "continue", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    assert!(wait_for_event(&mut reader, "terminated", 50));

    send_dap(&mut stdin, 9, "disconnect", None);
    let _ = read_dap(&mut reader);
    let status = child.wait().expect("failed to wait for child");
    assert!(status.success());

    let _ = std::fs::remove_dir_all(&dir);
}

/// Regression (DAP-1/2/3): after the program terminates, inspection requests
/// (stackTrace/scopes/variables) must return promptly with empty results rather
/// than hanging the session waiting on a VM that is no longer polling. Runs a
/// trivial program to completion (no breakpoints), then issues the requests and
/// asserts each gets a timely, well-formed response.
#[test]
fn test_dap_inspection_after_termination_does_not_hang() {
    let binary = sema_binary();
    let dir = unique_temp_dir("post_term");
    let program_path = dir.join("done.sema");
    std::fs::write(&program_path, "(+ 1 2)\n").unwrap();

    let mut child = Command::new(&binary)
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {binary}: {e}"));

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    let _ = read_dap(&mut reader).unwrap();
    let _ = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        2,
        "launch",
        Some(serde_json::json!({ "program": program_path.to_string_lossy() })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "launch");

    send_dap(&mut stdin, 3, "configurationDone", None);
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "configurationDone");

    // Program runs to completion with no breakpoints.
    assert!(
        wait_for_event(&mut reader, "terminated", 50),
        "should receive terminated event"
    );

    // Each inspection request must get a timely response (no hang) now that the
    // VM has stopped polling its command channel.
    for (seq, command, args) in [
        (10, "stackTrace", serde_json::json!({})),
        (11, "scopes", serde_json::json!({ "frameId": 0 })),
        (
            12,
            "variables",
            serde_json::json!({ "variablesReference": 1 }),
        ),
    ] {
        send_dap(&mut stdin, seq, command, Some(args));
        let resp = read_dap_timeout(&mut reader, Duration::from_secs(5))
            .unwrap_or_else(|| panic!("{command} after termination hung (no response)"));
        assert_eq!(resp["command"], command, "unexpected response: {resp}");
        assert_eq!(resp["success"], true, "{command} should succeed: {resp}");
    }

    send_dap(&mut stdin, 13, "disconnect", None);
    let _ = read_dap(&mut reader);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
