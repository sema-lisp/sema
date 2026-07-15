//! Acceptance gate for Task 05/06: `mcp/call` must run its blocking JSON-RPC
//! round trip OFF the VM thread when driven through the UNIFIED RUNTIME
//! (`eval_str_via_runtime`), so two `async/spawn`ed calls OVERLAP instead of
//! serializing on the VM thread.
//!
//! This mirrors `mcp_async_test.rs`'s concurrency shape (deterministic marker
//! files / a busy flag as ordering signals, never wall-clock thresholds) but
//! routes every eval through the runtime rather than the legacy cooperative
//! scheduler. The legacy path is covered by `mcp_async_test.rs`; this file is
//! the runtime-path oracle.

use sema::Interpreter;

/// Sema string-literal-encode a Rust string (JSON string syntax is a valid Sema
/// string literal), for interpolating a marker path / server script.
fn sema_str(s: &str) -> String {
    serde_json::to_string(s).expect("string encodes to JSON")
}

fn unique_temp_path(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "sema-mcp-runtime-{tag}-{}-{n}.marker",
        std::process::id()
    ))
}

// Server A withholds its `tools/call` reply until it observes a marker file
// that server B's handler touches — a one-directional signal proving B received
// (and answered) its own request while A's was still in flight. If the two
// spawned calls overlap, A sees the marker and returns "a-saw-marker"; a
// serialized/blocking implementation would run A to completion on the VM thread
// first (B's task never getting a chance to touch the marker), so A would time
// out with "a-timed-out-without-marker".
const SERVER_WITHHOLDS_UNTIL_MARKER: &str = r#"
import json, sys, os, time
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
marker = sys.argv[1]
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    r = json.loads(line); method = r.get("method"); rid = r.get("id")
    if rid is None:
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "serverInfo": {"name": "withholds", "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
            {"name": "wait_for_marker", "description": "wait",
             "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        deadline = time.time() + 10
        seen = False
        while time.time() < deadline:
            if os.path.exists(marker):
                seen = True
                break
            time.sleep(0.01)
        text = "a-saw-marker" if seen else "a-timed-out-without-marker"
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "content": [{"type": "text", "text": text}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "no"}})
"#;

const SERVER_TOUCHES_MARKER: &str = r#"
import json, sys
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
marker = sys.argv[1]
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    r = json.loads(line); method = r.get("method"); rid = r.get("id")
    if rid is None:
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "serverInfo": {"name": "touches", "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
            {"name": "touch_marker", "description": "touch",
             "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        open(marker, "w").close()
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "content": [{"type": "text", "text": "b-done"}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "no"}})
"#;

/// ACCEPTANCE GATE: two `async/spawn`ed `mcp/call`s to DIFFERENT connections,
/// driven through the unified runtime, overlap in flight. Server A can only
/// answer once server B (running concurrently) has answered its own call — so a
/// non-overlapping (VM-thread-blocking) implementation deadlocks A until its
/// 10s server-side timeout and returns "a-timed-out-without-marker".
#[test]
fn spawned_mcp_calls_overlap_through_runtime() {
    let marker = unique_temp_path("overlap");
    let _ = std::fs::remove_file(&marker);
    let marker_arg = sema_str(&marker.to_string_lossy());

    let interp = Interpreter::new();
    let a_encoded = sema_str(SERVER_WITHHOLDS_UNTIL_MARKER);
    let b_encoded = sema_str(SERVER_TOUCHES_MARKER);
    interp
        .eval_str_via_runtime(&format!(
            r#"(define a (mcp/connect {{:command "python3" :args ["-c" {a_encoded} {marker_arg}]}}))"#
        ))
        .expect("connect a");
    interp
        .eval_str_via_runtime(&format!(
            r#"(define b (mcp/connect {{:command "python3" :args ["-c" {b_encoded} {marker_arg}]}}))"#
        ))
        .expect("connect b");

    let program = r#"
        (let ((ta (async/spawn (fn () (mcp/call a "wait_for_marker" {}))))
              (tb (async/spawn (fn () (mcp/call b "touch_marker" {})))))
          (list (async/await ta) (async/await tb)))
    "#;
    let result = interp
        .eval_str_via_runtime(program)
        .expect("both connections' calls complete without deadlock");
    let items = result.as_seq().expect("a list of two results");
    let texts: Vec<&str> = items.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        texts.contains(&"a-saw-marker"),
        "server A must have observed B's marker before answering (proves in-flight \
         overlap through the runtime); got {texts:?}"
    );
    assert!(texts.contains(&"b-done"), "got {texts:?}");

    interp.eval_str_via_runtime(r#"(mcp/close a)"#).ok();
    interp.eval_str_via_runtime(r#"(mcp/close b)"#).ok();
    let _ = std::fs::remove_file(&marker);
}

// A per-connection busy flag + incrementing counter: a second `tools/call`
// arriving before the first is answered is a hard server-side error — proof the
// client serialized requests on this ONE connection, exactly as the MCP wire
// protocol requires.
const BUSY_COUNTER_SERVER: &str = r#"
import json, sys, time
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
busy = False
counter = 0
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    r = json.loads(line); method = r.get("method"); rid = r.get("id")
    if rid is None:
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "serverInfo": {"name": "busy-counter", "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
            {"name": "count", "description": "increment",
             "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        if busy:
            send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32000,
                  "message": "overlap detected: a second request arrived before the first response was sent"}})
            continue
        busy = True
        time.sleep(0.05)
        counter += 1
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "content": [{"type": "text", "text": "call-%d" % counter}], "isError": False}})
        busy = False
    else:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "no"}})
"#;

/// SAME connection: two concurrent `mcp/call`s must SERIALIZE through the
/// runtime (the JSON-RPC pipe is serial). A blocking-overlap on one connection
/// would trip the server's busy flag; a lost queue wakeup would hang. Both must
/// complete, exactly once each, strictly sequentially.
#[test]
fn same_connection_mcp_calls_serialize_through_runtime() {
    let interp = Interpreter::new();
    let encoded = sema_str(BUSY_COUNTER_SERVER);
    interp
        .eval_str_via_runtime(&format!(
            r#"(define server (mcp/connect {{:command "python3" :args ["-c" {encoded}]}}))"#
        ))
        .expect("connect");

    let program = r#"
        (let ((t1 (async/spawn (fn () (mcp/call server "count" {}))))
              (t2 (async/spawn (fn () (mcp/call server "count" {})))))
          (list (async/await t1) (async/await t2)))
    "#;
    let result = interp.eval_str_via_runtime(program).expect(
        "both queued calls must succeed — a server-side overlap error means the \
                 client sent a second request before the first was answered",
    );
    let items = result.as_seq().expect("a list of two results");
    let mut vals: Vec<&str> = items.iter().map(|v| v.as_str().unwrap()).collect();
    vals.sort_unstable();
    assert_eq!(vals, vec!["call-1", "call-2"]);

    interp.eval_str_via_runtime(r#"(mcp/close server)"#).ok();
}
