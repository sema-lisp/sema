//! DAP integration tests.
//!
//! NOTE: these spawn the compiled `sema` binary (package `sema-lang`) and drive
//! it over the DAP protocol. `cargo test -p sema-dap` in isolation does NOT
//! rebuild that binary, so it can run against a stale `sema` and give false
//! passes after a change to sema-vm/sema-eval/sema-dap. Run these via the full
//! `cargo test` (which builds the workspace, incl. the binary) or do
//! `cargo build -p sema-lang` first. (Found via mutation testing.)

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
fn test_dap_conditional_breakpoint_only_stops_when_truthy() {
    let binary = sema_binary();

    let dir = unique_temp_dir("cond_bp");
    let program_path = dir.join("cond.sema");
    // `f` is called three times; the breakpoint inside its body (line 2) carries
    // the condition `(= n 2)`, so it must fire only on the second call.
    std::fs::write(
        &program_path,
        "(define (f n)\n  (+ n 100))\n(f 1)\n(f 2)\n(f 3)\n",
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
    assert_eq!(init["body"]["supportsConditionalBreakpoints"], true);
    let _event = read_dap(&mut reader).unwrap();

    // Conditional breakpoint on the function body line.
    send_dap(
        &mut stdin,
        2,
        "setBreakpoints",
        Some(serde_json::json!({
            "source": { "path": program_path.to_string_lossy() },
            "breakpoints": [{ "line": 2, "condition": "(= n 2)" }],
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "setBreakpoints");
    assert_eq!(resp["success"], true);

    send_dap(
        &mut stdin,
        3,
        "launch",
        Some(serde_json::json!({ "program": program_path.to_string_lossy() })),
    );
    let _resp = read_dap(&mut reader).unwrap();

    send_dap(&mut stdin, 4, "configurationDone", None);
    let _resp = read_dap(&mut reader).unwrap();

    // First stop must be when n == 2 (the f(1) call is skipped by the condition).
    wait_for_event_message(&mut reader, "stopped", 50)
        .expect("conditional breakpoint should stop when condition holds");

    send_dap(&mut stdin, 5, "stackTrace", Some(serde_json::json!({})));
    let resp = read_dap(&mut reader).unwrap();
    let frame_id = resp["body"]["stackFrames"]
        .as_array()
        .unwrap()
        .iter()
        .find(|frame| frame["name"] == "f")
        .and_then(|frame| frame["id"].as_u64())
        .expect("stack trace should include f frame");

    // Confirm n is indeed 2 at this stop.
    send_dap(
        &mut stdin,
        6,
        "evaluate",
        Some(serde_json::json!({
            "expression": "n",
            "frameId": frame_id,
            "context": "watch",
        })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["success"], true);
    assert_eq!(
        resp["body"]["result"], "2",
        "conditional breakpoint stopped with n == 2"
    );

    // Continue: the f(3) call must NOT stop (condition false), so the program
    // runs to termination.
    send_dap(&mut stdin, 7, "continue", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    assert!(
        wait_for_event(&mut reader, "terminated", 50),
        "program should terminate without stopping again (f(3) condition is false)"
    );

    send_dap(&mut stdin, 8, "disconnect", None);
    let _ = read_dap(&mut reader);
    let status = child.wait().expect("failed to wait for child");
    assert!(status.success());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_exception_breakpoint_stops_on_uncaught_error() {
    let binary = sema_binary();

    let dir = unique_temp_dir("exc_bp");
    let program_path = dir.join("boom.sema");
    // Calls a non-existent function: a runtime error that is never caught.
    std::fs::write(&program_path, "(define x 1)\n(this-fn-does-not-exist x)\n").unwrap();

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
    assert_eq!(init["body"]["supportsExceptionInfoRequest"], true);
    assert_eq!(
        init["body"]["exceptionBreakpointFilters"][0]["filter"],
        "uncaught"
    );
    let _event = read_dap(&mut reader).unwrap();

    // Enable the uncaught-exception filter.
    send_dap(
        &mut stdin,
        2,
        "setExceptionBreakpoints",
        Some(serde_json::json!({ "filters": ["uncaught"] })),
    );
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "setExceptionBreakpoints");
    assert_eq!(resp["success"], true);

    send_dap(
        &mut stdin,
        3,
        "launch",
        Some(serde_json::json!({ "program": program_path.to_string_lossy() })),
    );
    let _resp = read_dap(&mut reader).unwrap();

    send_dap(&mut stdin, 4, "configurationDone", None);
    let _resp = read_dap(&mut reader).unwrap();

    // Must stop on the uncaught error with reason "exception".
    let stopped = wait_for_event_message(&mut reader, "stopped", 50)
        .expect("should stop on uncaught exception");
    assert_eq!(stopped["body"]["reason"], "exception");

    // exceptionInfo must report the error.
    send_dap(&mut stdin, 5, "exceptionInfo", Some(serde_json::json!({})));
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "exceptionInfo");
    assert_eq!(resp["success"], true);
    assert_eq!(resp["body"]["breakMode"], "unhandled");
    assert!(
        resp["body"]["description"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "exceptionInfo should carry a non-empty description"
    );

    // Continue past the exception stop: the session terminates.
    send_dap(&mut stdin, 6, "continue", Some(serde_json::json!({})));
    let _resp = read_dap(&mut reader).unwrap();
    assert!(
        wait_for_event(&mut reader, "terminated", 50),
        "session should terminate after the uncaught exception"
    );

    send_dap(&mut stdin, 7, "disconnect", None);
    let _ = read_dap(&mut reader);
    let _ = child.wait();

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_exception_breakpoint_skips_caught_errors() {
    // The `uncaught` filter must NOT stop on an error that is caught by `try`.
    // The program catches its error and runs to completion, so the session
    // should terminate normally — never emitting an exception stop.
    let binary = sema_binary();

    let dir = unique_temp_dir("exc_bp_caught");
    let program_path = dir.join("caught.sema");
    std::fs::write(
        &program_path,
        "(try (this-fn-does-not-exist) (catch e \"caught\"))\n(println \"done\")\n",
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
    let _init = read_dap(&mut reader).unwrap();
    let _event = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        2,
        "setExceptionBreakpoints",
        Some(serde_json::json!({ "filters": ["uncaught"] })),
    );
    let _resp = read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        3,
        "launch",
        Some(serde_json::json!({ "program": program_path.to_string_lossy() })),
    );
    let _resp = read_dap(&mut reader).unwrap();

    send_dap(&mut stdin, 4, "configurationDone", None);
    let _resp = read_dap(&mut reader).unwrap();

    // The caught error must not park the program at an exception stop; it runs
    // to completion. If the filter wrongly fired, `terminated` would never come.
    assert!(
        wait_for_event(&mut reader, "terminated", 50),
        "caught error must not trigger the uncaught-exception breakpoint"
    );

    send_dap(&mut stdin, 5, "disconnect", None);
    let _ = read_dap(&mut reader);
    let _ = child.wait();

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

/// A debugged program that uses async must run to termination: the DAP backend
/// initializes the async scheduler before `execute_debug`, so `(await (async
/// ...))` resolves instead of erroring with "no async scheduler registered".
#[test]
#[ignore = "async debugging pending runtime cooperative-debug mode — see docs/deferred.md (ASYNC-DEBUG-1)"]
fn test_dap_async_program_runs_to_termination() {
    let binary = sema_binary();

    let dir = unique_temp_dir("async");
    let program_path = dir.join("test.sema");
    std::fs::write(&program_path, "(println (await (async (+ 1 2))))\n").unwrap();

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

    // ConfigurationDone — triggers execution.
    send_dap(&mut stdin, 3, "configurationDone", None);
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "configurationDone");
    assert_eq!(resp["success"], true);

    // Scan messages until terminated, collecting any stdout output. The async
    // program must complete (no "no async scheduler registered" error on stderr)
    // and print the resolved value "3".
    let mut saw_terminated = false;
    let mut stdout_text = String::new();
    for _ in 0..50 {
        let Some(msg) = read_dap(&mut reader) else {
            break;
        };
        if msg["type"] == "event" {
            match msg["event"].as_str() {
                Some("output") => {
                    let category = msg["body"]["category"].as_str().unwrap_or("");
                    if let Some(out) = msg["body"]["output"].as_str() {
                        if category != "stderr" {
                            stdout_text.push_str(out);
                        }
                        assert!(
                            !out.contains("no async scheduler registered"),
                            "async program emitted a scheduler error: {out}"
                        );
                    }
                }
                Some("terminated") => {
                    saw_terminated = true;
                    break;
                }
                _ => {}
            }
        }
    }
    assert!(saw_terminated, "should receive terminated event");
    assert!(
        stdout_text.contains('3'),
        "async program should print resolved value 3, got: {stdout_text:?}"
    );

    send_dap(&mut stdin, 4, "disconnect", None);
    let _ = read_dap(&mut reader);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Drive the init → setBreakpoints → launch → configurationDone handshake and return
/// once the program is parked at its first `stopped` event. Returns the live child +
/// its stdin/reader so the test can step/inspect.
fn session_stopped_at(
    program_path: &std::path::Path,
    breakpoint_lines: &[u32],
) -> (
    std::process::Child,
    std::process::ChildStdin,
    BufReader<std::process::ChildStdout>,
) {
    let mut child = Command::new(sema_binary())
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dap");
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    read_dap(&mut reader).unwrap(); // response
    read_dap(&mut reader).unwrap(); // initialized event

    let bps: Vec<_> = breakpoint_lines
        .iter()
        .map(|l| serde_json::json!({ "line": l }))
        .collect();
    send_dap(
        &mut stdin,
        2,
        "setBreakpoints",
        Some(serde_json::json!({
        "source": { "path": program_path.to_string_lossy() }, "breakpoints": bps })),
    );
    read_dap(&mut reader).unwrap();

    send_dap(
        &mut stdin,
        3,
        "launch",
        Some(serde_json::json!({ "program": program_path.to_string_lossy() })),
    );
    read_dap(&mut reader).unwrap();
    send_dap(&mut stdin, 4, "configurationDone", None);
    read_dap(&mut reader).unwrap();

    assert!(
        wait_for_event(&mut reader, "stopped", 50),
        "should stop at the breakpoint"
    );
    (child, stdin, reader)
}

fn stack_frames(
    stdin: &mut impl Write,
    reader: &mut BufReader<impl Read>,
    seq: u64,
) -> Vec<serde_json::Value> {
    send_dap(stdin, seq, "stackTrace", Some(serde_json::json!({})));
    let resp = read_dap(reader).unwrap();
    assert_eq!(resp["command"], "stackTrace");
    resp["body"]["stackFrames"].as_array().unwrap().clone()
}

const STEP_PROG: &str = "(define (f n)\n  (+ n 1))\n(define a 1)\n(f 10)\n(define b 2)\n";

#[test]
fn test_dap_breakpoint_stops_on_requested_line() {
    // Pin the 1-based stop LINE (not just "some frame"): a 1-vs-0-based regression in the
    // stop location would otherwise pass.
    let dir = unique_temp_dir("bp-line");
    let p = dir.join("p.sema");
    std::fs::write(&p, STEP_PROG).unwrap();
    let (mut child, mut stdin, mut reader) = session_stopped_at(&p, &[4]);
    let frames = stack_frames(&mut stdin, &mut reader, 5);
    assert_eq!(frames[0]["line"], 4, "must stop on the requested line 4");
    send_dap(&mut stdin, 9, "disconnect", None);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_next_steps_over_call_stays_in_frame() {
    // Step-over at the `(f 10)` call (line 4) must land on line 5 in the SAME top-level
    // frame — never inside f's body (line 2).
    let dir = unique_temp_dir("step-over");
    let p = dir.join("p.sema");
    std::fs::write(&p, STEP_PROG).unwrap();
    let (mut child, mut stdin, mut reader) = session_stopped_at(&p, &[4]);

    send_dap(&mut stdin, 5, "next", None);
    read_dap(&mut reader).unwrap(); // ack
    let stopped =
        wait_for_event_message(&mut reader, "stopped", 50).expect("step-over stops again");
    assert_eq!(stopped["body"]["reason"], "step");

    let frames = stack_frames(&mut stdin, &mut reader, 6);
    assert_eq!(
        frames.len(),
        1,
        "step-over must not descend into f: {frames:?}"
    );
    assert_eq!(
        frames[0]["line"], 5,
        "step-over lands on the next top-level line"
    );

    send_dap(&mut stdin, 9, "disconnect", None);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_step_in_descends_into_callee() {
    // Step-in at the `(f 10)` call must descend into f (2 frames, top frame inside the
    // callee body on line 2).
    let dir = unique_temp_dir("step-in");
    let p = dir.join("p.sema");
    std::fs::write(&p, STEP_PROG).unwrap();
    let (mut child, mut stdin, mut reader) = session_stopped_at(&p, &[4]);

    send_dap(&mut stdin, 5, "stepIn", None);
    read_dap(&mut reader).unwrap(); // ack
    let stopped = wait_for_event_message(&mut reader, "stopped", 50).expect("step-in stops");
    assert_eq!(stopped["body"]["reason"], "step");

    let frames = stack_frames(&mut stdin, &mut reader, 6);
    assert_eq!(frames.len(), 2, "step-in must descend into f: {frames:?}");
    assert_eq!(frames[0]["line"], 2, "top frame is inside f's body");

    send_dap(&mut stdin, 9, "disconnect", None);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dap_stop_on_entry_pauses_before_running() {
    // stopOnEntry is wired but was untested — a regression would silently never pause at
    // entry. Launch with no breakpoints + stopOnEntry:true → a stopped event must arrive
    // before terminated; continue then runs to completion.
    let dir = unique_temp_dir("entry");
    let p = dir.join("p.sema");
    std::fs::write(&p, "(+ 1 2)\n").unwrap();

    let mut child = Command::new(sema_binary())
        .arg("dap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    send_dap(&mut stdin, 1, "initialize", Some(serde_json::json!({})));
    read_dap(&mut reader).unwrap();
    read_dap(&mut reader).unwrap();
    send_dap(
        &mut stdin,
        2,
        "launch",
        Some(serde_json::json!({
        "program": p.to_string_lossy(), "stopOnEntry": true })),
    );
    // The launch response must correlate to its request seq.
    let resp = read_dap(&mut reader).unwrap();
    assert_eq!(resp["command"], "launch");
    assert_eq!(
        resp["request_seq"], 2,
        "response must correlate to the request seq"
    );
    send_dap(&mut stdin, 3, "configurationDone", None);
    read_dap(&mut reader).unwrap();

    assert!(
        wait_for_event(&mut reader, "stopped", 50),
        "stopOnEntry must pause at entry"
    );

    send_dap(&mut stdin, 4, "continue", Some(serde_json::json!({})));
    read_dap(&mut reader).unwrap();
    assert!(
        wait_for_event(&mut reader, "terminated", 50),
        "continue runs to completion"
    );

    send_dap(&mut stdin, 5, "disconnect", None);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
