//! End-to-end LSP round-trips against a real `sema lsp` process over stdio JSON-RPC.
//!
//! Spawns the built `sema` binary and exercises `initialize` → `didOpen` → `formatting` and
//! `selectionRange`, asserting real protocol responses. The test locates the `sema` binary next to
//! the test runner (`target/<profile>/sema`) and skips gracefully if it hasn't been built — so
//! `cargo test -p sema-lsp` is a no-op for this file unless the binary exists, while CI (which
//! builds the workspace) runs it for real.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

use serde_json::{json, Value};

/// Find the `sema` binary alongside the test runner (e.g. `target/debug/sema`).
fn find_sema_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    for dir in exe.ancestors() {
        for name in ["sema", "sema.exe"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn send(stdin: &mut ChildStdin, msg: &Value) {
    let body = serde_json::to_string(msg).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    stdin.flush().unwrap();
}

fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None; // EOF
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().ok()?;
        }
    }
    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

/// Read messages until the response with `id` arrives, skipping notifications (e.g. diagnostics).
fn wait_for_response(reader: &mut impl BufRead, id: i64) -> Value {
    loop {
        let msg = read_message(reader).expect("server closed the connection unexpectedly");
        if msg.get("id").and_then(Value::as_i64) == Some(id) {
            return msg;
        }
    }
}

fn did_open(stdin: &mut ChildStdin, uri: &str, text: &str) {
    send(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": { "uri": uri, "languageId": "sema", "version": 1, "text": text }
            }
        }),
    );
}

struct ServerGuard(Child);
impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawns `sema lsp` and completes the `initialize`/`initialized` handshake.
fn start_initialized_server(
    sema: &std::path::Path,
) -> (
    ServerGuard,
    ChildStdin,
    BufReader<std::process::ChildStdout>,
) {
    // Run from an empty temp dir so the `initialized` workspace scan finds nothing (fast + stable).
    let mut child = Command::new(sema)
        .arg("lsp")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `sema lsp`");

    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let guard = ServerGuard(child);

    send(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
    );
    wait_for_response(&mut reader, 1);
    send(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
    );

    (guard, stdin, reader)
}

/// Polls the child until it exits or `timeout` elapses.
fn wait_for_exit(
    child: &mut Child,
    timeout: std::time::Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("wait on `sema lsp`") {
            return Some(status);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn lsp_formatting_and_selection_range_round_trip() {
    let Some(sema) = find_sema_binary() else {
        eprintln!("skipping lsp_e2e: `sema` binary not found next to test runner — build it first");
        return;
    };

    // Run from an empty temp dir so the `initialized` workspace scan finds nothing (fast + stable).
    let workdir = std::env::temp_dir();
    let mut child = Command::new(&sema)
        .arg("lsp")
        .current_dir(&workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `sema lsp`");

    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut guard = ServerGuard(child);

    // initialize
    send(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} }
        }),
    );
    let init = wait_for_response(&mut reader, 1);
    let caps = &init["result"]["capabilities"];
    assert!(
        !caps["documentFormattingProvider"].is_null(),
        "formatting capability missing"
    );
    assert!(
        !caps["selectionRangeProvider"].is_null(),
        "selectionRange capability missing"
    );
    assert!(
        !caps["callHierarchyProvider"].is_null(),
        "callHierarchy capability missing"
    );
    assert!(
        !caps["documentLinkProvider"].is_null(),
        "documentLink capability missing"
    );

    send(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
    );

    // ── formatting ──
    let fmt_uri = "file:///e2e/fmt.sema";
    did_open(&mut stdin, fmt_uri, "(define   x    42)");
    send(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/formatting",
            "params": {
                "textDocument": { "uri": fmt_uri },
                "options": { "tabSize": 2, "insertSpaces": true }
            }
        }),
    );
    let fmt = wait_for_response(&mut reader, 2);
    let edits = fmt["result"]
        .as_array()
        .expect("formatting returns an array of edits");
    assert_eq!(edits.len(), 1, "expected one full-document edit");
    let new_text = edits[0]["newText"].as_str().unwrap();
    assert!(
        new_text.contains("(define x 42)"),
        "formatter normalized the form: {new_text:?}"
    );

    // ── selection range ──
    let sel_uri = "file:///e2e/sel.sema";
    did_open(&mut stdin, sel_uri, "(define x (+ a b))");
    send(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/selectionRange",
            "params": {
                "textDocument": { "uri": sel_uri },
                "positions": [ { "line": 0, "character": 13 } ]
            }
        }),
    );
    let sel = wait_for_response(&mut reader, 3);
    let ranges = sel["result"]
        .as_array()
        .expect("selectionRange returns an array");
    assert_eq!(
        ranges.len(),
        1,
        "one selection range per requested position"
    );
    // Innermost has an enclosing parent (the structural expansion chain).
    assert!(
        !ranges[0]["parent"].is_null(),
        "expected a parent selection range"
    );

    // shutdown, await the response, then exit — the server must terminate
    // promptly with code 0 (LSP lifecycle; regression for the exit hang).
    send(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
    );
    let _ = wait_for_response(&mut reader, 4);
    send(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    );
    let status = wait_for_exit(&mut guard.0, std::time::Duration::from_secs(3))
        .expect("server did not exit within 3s of shutdown + exit");
    assert_eq!(status.code(), Some(0), "clean shutdown/exit must exit 0");
}

/// Regression: `shutdown` + `exit` sent back-to-back — without reading the
/// shutdown response first — must terminate the process promptly with exit
/// code 0. tower-lsp's own exit handling never ends the process (tower-lsp
/// issue #399), so the stdio transport watches the lifecycle frames itself.
#[test]
fn lsp_exits_zero_on_immediate_shutdown_exit() {
    let Some(sema) = find_sema_binary() else {
        eprintln!("skipping lsp_e2e: `sema` binary not found next to test runner — build it first");
        return;
    };
    let (mut guard, mut stdin, _reader) = start_initialized_server(&sema);

    send(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null }),
    );
    send(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    );

    let started = std::time::Instant::now();
    let status = wait_for_exit(&mut guard.0, std::time::Duration::from_secs(3))
        .expect("server did not exit within 3s of back-to-back shutdown + exit");
    eprintln!("shutdown+exit terminated in {:?}", started.elapsed());
    assert_eq!(status.code(), Some(0), "shutdown-then-exit must exit 0");
}

/// Regression: an `exit` notification without a prior `shutdown` request must
/// terminate the process promptly with exit code 1 (per the LSP spec).
#[test]
fn lsp_exits_one_on_exit_without_shutdown() {
    let Some(sema) = find_sema_binary() else {
        eprintln!("skipping lsp_e2e: `sema` binary not found next to test runner — build it first");
        return;
    };
    let (mut guard, mut stdin, _reader) = start_initialized_server(&sema);

    send(
        &mut stdin,
        &json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    );

    let started = std::time::Instant::now();
    let status = wait_for_exit(&mut guard.0, std::time::Duration::from_secs(3))
        .expect("server did not exit within 3s of exit-without-shutdown");
    eprintln!(
        "exit-without-shutdown terminated in {:?}",
        started.elapsed()
    );
    assert_eq!(status.code(), Some(1), "exit without shutdown must exit 1");
}
