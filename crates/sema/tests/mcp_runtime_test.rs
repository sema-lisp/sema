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

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use sema::Interpreter;
use sema_core::{Sandbox, Value};
use sema_eval::Interpreter as EvalInterpreter;
use sema_vm::runtime::RootOptions;

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

struct Markers {
    release: std::path::PathBuf,
    entered: std::path::PathBuf,
    timed_out: std::path::PathBuf,
    finished: std::path::PathBuf,
}

impl Markers {
    fn new(tag: &str) -> Self {
        Self {
            release: unique_temp_path(&format!("{tag}-release")),
            entered: unique_temp_path(&format!("{tag}-entered")),
            timed_out: unique_temp_path(&format!("{tag}-timed-out")),
            finished: unique_temp_path(&format!("{tag}-finished")),
        }
    }

    fn script_args(&self) -> String {
        format!(
            "{} {} {} {}",
            sema_str(&self.release.to_string_lossy()),
            sema_str(&self.entered.to_string_lossy()),
            sema_str(&self.timed_out.to_string_lossy()),
            sema_str(&self.finished.to_string_lossy()),
        )
    }
}

impl Drop for Markers {
    fn drop(&mut self) {
        for path in [
            &self.release,
            &self.entered,
            &self.timed_out,
            &self.finished,
        ] {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn wait_for_path(path: &std::path::Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    path.exists()
}

fn mcp_eval_interpreter() -> EvalInterpreter {
    let interp = EvalInterpreter::new();
    sema_mcp::register_mcp_builtins(&interp.global_env, &Sandbox::allow_all());
    interp
}

const DELAYED_STDIO_SERVER: &str = r#"
import json, os, select, sys, time

mode, release, entered, timed_out, finished = sys.argv[1:6]
stdin_fd = sys.stdin.fileno()

def touch(path):
    open(path, "w").close()

def wait_for_release():
    with open(entered, "w") as marker:
        marker.write(str(os.getpid()))
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if os.path.exists(release):
            touch(finished)
            return
        readable, _, _ = select.select([stdin_fd], [], [], 0)
        if readable and os.read(stdin_fd, 1) == b"":
            touch(finished)
            raise SystemExit(0)
        time.sleep(0.005)
    touch(timed_out)
    touch(finished)

def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()

def read_line():
    line = bytearray()
    while True:
        byte = os.read(stdin_fd, 1)
        if byte == b"":
            return None
        if byte == b"\n":
            return line.decode()
        line.extend(byte)

while True:
    line = read_line()
    if line is None:
        break
    line = line.strip()
    if not line:
        continue
    request = json.loads(line)
    method = request.get("method")
    request_id = request.get("id")
    if request_id is None:
        continue
    if method == "initialize":
        if mode == "connect":
            wait_for_release()
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "serverInfo": {"name": "delayed", "version": "1"}}})
    elif method == "tools/list":
        if mode == "tools":
            wait_for_release()
        if mode == "tools-error":
            send({"jsonrpc": "2.0", "id": request_id,
                  "error": {"code": -32042, "message": "tools exploded"}})
            continue
        send({"jsonrpc": "2.0", "id": request_id, "result": {"tools": [
            {"name": "echo", "description": "Echo a string",
             "inputSchema": {"type": "object",
                             "properties": {"text": {"type": "string"}},
                             "required": ["text"]}}
        ]}})
    elif method == "tools/call":
        if mode == "call":
            wait_for_release()
        arguments = request.get("params", {}).get("arguments", {})
        send({"jsonrpc": "2.0", "id": request_id, "result": {
            "content": [{"type": "text", "text": arguments.get("text", "")}],
            "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": request_id,
              "error": {"code": -32601, "message": "Method not found"}})
"#;

fn delayed_stdio_config(mode: &str, markers: &Markers) -> String {
    let script = sema_str(DELAYED_STDIO_SERVER);
    format!(
        r#"{{:command "python3" :args ["-c" {script} {} {}]}}"#,
        sema_str(mode),
        markers.script_args(),
    )
}

const DELAYED_HTTP_SERVER: &str = r#"
import json, os, select, socket, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

release, entered, timed_out, finished = sys.argv[1:5]

def touch(path):
    open(path, "w").close()

def peer_closed(connection):
    readable, _, _ = select.select([connection], [], [], 0)
    if not readable:
        return False
    try:
        return connection.recv(1, socket.MSG_PEEK) == b""
    except (ConnectionResetError, OSError):
        return True

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, body=b"", headers=None):
        self.send_response(status)
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if body:
            self.wfile.write(body)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b""
        request = json.loads(raw) if raw else {}
        request_id = request.get("id")
        if request_id is None:
            self.reply(202)
            return
        method = request.get("method")
        if method == "initialize":
            result = {"protocolVersion": "2025-11-25", "capabilities": {},
                      "serverInfo": {"name": "delayed-http", "version": "1"}}
        elif method == "tools/list":
            result = {"tools": []}
        else:
            result = {}
        body = json.dumps({"jsonrpc": "2.0", "id": request_id,
                           "result": result}).encode()
        self.reply(200, body, {"Content-Type": "application/json",
                               "Mcp-Session-Id": "delayed-session"})

    def do_DELETE(self):
        touch(entered)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if os.path.exists(release):
                touch(finished)
                self.reply(200)
                return
            if peer_closed(self.connection):
                touch(finished)
                return
            time.sleep(0.005)
        touch(timed_out)
        touch(finished)
        self.reply(200)

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()
"#;

struct HttpServerGuard {
    child: Child,
}

impl Drop for HttpServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_delayed_http_server(markers: &Markers) -> (HttpServerGuard, u16) {
    let mut child = Command::new("python3")
        .args([
            "-c",
            DELAYED_HTTP_SERVER,
            &markers.release.to_string_lossy(),
            &markers.entered.to_string_lossy(),
            &markers.timed_out.to_string_lossy(),
            &markers.finished.to_string_lossy(),
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn delayed HTTP MCP server");
    let stdout = child.stdout.take().expect("HTTP server stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read HTTP server port");
    let port = line.trim().parse().expect("HTTP server port is a u16");
    (HttpServerGuard { child }, port)
}

#[derive(Clone, Copy)]
enum CancellationResourceOracle {
    StdioServerExit,
    HttpPeerDisconnect,
}

fn stdio_server_exited(entered: &std::path::Path) -> bool {
    let Ok(pid) = std::fs::read_to_string(entered) else {
        return false;
    };
    let probe = r#"
import os, sys
try:
    os.kill(int(sys.argv[1]), 0)
except OSError:
    raise SystemExit(1)
"#;
    !Command::new("python3")
        .args(["-c", probe, pid.trim()])
        .status()
        .is_ok_and(|status| status.success())
}

fn assert_cancelled_before_server_fallback(
    interp: &EvalInterpreter,
    program: &str,
    markers: &Markers,
    oracle: CancellationResourceOracle,
) {
    let root = interp
        .submit_str(program, RootOptions::default())
        .expect("MCP operation root submits");
    let root_id = root.id();
    let command = interp.command_handle();
    let entered = markers.entered.clone();
    let emergency_release = markers.release.clone();
    let canceller = thread::spawn(move || {
        let saw_entry = wait_for_path(&entered, Duration::from_secs(6));
        if !saw_entry {
            let _ = std::fs::write(emergency_release, b"release");
            return (false, false);
        }
        (true, command.cancel_root(root_id))
    });

    let result = interp.drive_until_settled(&root);
    let (saw_entry, cancel_accepted) = canceller.join().expect("canceller thread completes");
    let peer_finished = wait_for_path(&markers.finished, Duration::from_secs(1));
    let resource_interrupted = peer_finished
        || matches!(oracle, CancellationResourceOracle::StdioServerExit)
            && stdio_server_exited(&markers.entered);
    let server_fallback_ran = markers.timed_out.exists();
    if !resource_interrupted {
        // Cleanup for the RED/failure path only. A passing cancellation must
        // make the transport peer close without this release marker.
        let _ = std::fs::write(&markers.release, b"release");
        let _ = wait_for_path(&markers.finished, Duration::from_secs(6));
    }

    assert!(
        saw_entry,
        "the MCP operation never reached the delayed server"
    );
    assert!(cancel_accepted, "the live MCP root rejected cancellation");
    assert!(result.is_err(), "the cancelled MCP root returned a value");
    assert!(
        !server_fallback_ran,
        "runtime cancellation was not serviced until the server's bounded fallback released the operation"
    );
    assert!(
        resource_interrupted,
        "cancellation settled the root without interrupting the MCP transport resource"
    );
    assert_eq!(
        interp.runtime_live_task_count(),
        0,
        "cancelled MCP operation left a runtime task behind"
    );
    assert_eq!(
        interp.runtime_resource_gate_count(),
        0,
        "cancelled MCP terminal paths must close their connection gate"
    );
}

fn generated_echo_handler(defs: &Value) -> Value {
    defs.as_seq()
        .expect("mcp/tools->sema returns tool definitions")
        .iter()
        .find_map(|value| {
            value
                .as_tool_def_rc()
                .filter(|tool| tool.name == "echo")
                .map(|tool| tool.handler.clone())
        })
        .expect("delayed server exposes an echo handler")
}

fn install_generated_echo_handler(interp: &Interpreter, binding: &str) {
    let defs = interp
        .eval_str_via_runtime("(mcp/tools->sema server)")
        .expect("generate MCP-backed tool definitions");
    interp
        .global_env()
        .set_str(binding, generated_echo_handler(&defs));
}

fn install_generated_echo_handler_for_eval(interp: &EvalInterpreter, binding: &str) {
    let defs = interp
        .eval_str_via_runtime("(mcp/tools->sema server)")
        .expect("generate MCP-backed tool definitions");
    interp
        .global_env
        .set_str(binding, generated_echo_handler(&defs));
}

fn assert_connection_tombstoned(interp: &EvalInterpreter, expected_reason: &str) {
    let error = interp
        .eval_str_via_runtime("(mcp/tools server)")
        .expect_err("a cancelled checked-out connection must be tombstoned");
    let message = error.to_string();
    assert!(message.contains("mcp connection lost"), "{message}");
    assert!(message.contains(expected_reason), "{message}");
    assert_eq!(
        error.hint(),
        Some("reconnect with mcp/connect (or the workflow's :mcp manifest) and retry")
    );
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

#[test]
fn runtime_mcp_connect_wait_allows_sibling_progress() {
    let markers = Markers::new("connect-progress");
    let config = delayed_stdio_config("connect", &markers);
    let release = sema_str(&markers.release.to_string_lossy());
    let interp = Interpreter::new();
    let result = interp
        .eval_str_via_runtime(&format!(
            r#"
            (let ((operation (async/spawn (fn () (mcp/connect {config}))))
                  (sibling (async/spawn (fn ()
                    (file/write {release} "release")
                    :sibling))))
              (list (async/await operation) (async/await sibling)))
            "#
        ))
        .expect("mcp/connect and its sibling complete");
    let values = result.as_seq().expect("connect result and sibling marker");
    assert_eq!(values.len(), 2);
    assert!(values[0].as_str().is_some(), "connect returns a handle");
    assert_eq!(values[1].as_keyword().as_deref(), Some("sibling"));
    assert!(markers.entered.exists(), "server delayed initialize");
    assert!(
        !markers.timed_out.exists(),
        "connect blocked the VM until the server's fallback fired"
    );
    sema_mcp::close_handle(&values[0]);
}

#[test]
fn runtime_mcp_tools_wait_allows_sibling_progress() {
    let markers = Markers::new("tools-progress");
    let config = delayed_stdio_config("tools", &markers);
    let release = sema_str(&markers.release.to_string_lossy());
    let interp = Interpreter::new();
    interp
        .eval_str_via_runtime(&format!("(define server (mcp/connect {config}))"))
        .expect("connect delayed tools server");
    let result = interp
        .eval_str_via_runtime(&format!(
            r#"
            (let ((operation (async/spawn (fn () (mcp/tools server))))
                  (sibling (async/spawn (fn ()
                    (file/write {release} "release")
                    :sibling))))
              (list (async/await operation) (async/await sibling)))
            "#
        ))
        .expect("mcp/tools and its sibling complete");
    let values = result.as_seq().expect("tools result and sibling marker");
    let tools = values[0].as_seq().expect("mcp/tools returns a list");
    assert_eq!(tools.len(), 1);
    assert_eq!(values[1].as_keyword().as_deref(), Some("sibling"));
    assert!(markers.entered.exists(), "server delayed tools/list");
    assert!(
        !markers.timed_out.exists(),
        "tools/list blocked the VM until the server's fallback fired"
    );
    assert_eq!(
        interp.runtime_resource_gate_count(),
        1,
        "mcp/tools lazily creates one connection gate"
    );
    interp.eval_str_via_runtime("(mcp/close server)").ok();
    assert_eq!(
        interp.runtime_resource_gate_count(),
        0,
        "mcp/close must close the connection gate"
    );
}

#[test]
fn public_close_handle_closes_a_runtime_created_connection_gate() {
    let markers = Markers::new("host-close-gate");
    let config = delayed_stdio_config("immediate", &markers);
    let interp = mcp_eval_interpreter();
    interp
        .eval_str_via_runtime(&format!("(define server (mcp/connect {config}))"))
        .expect("connect immediate MCP server");
    interp
        .eval_str_via_runtime("(mcp/tools server)")
        .expect("create the connection gate through mcp/tools");
    let handle = interp
        .eval_str_via_runtime("server")
        .expect("read opaque connection handle");
    assert_eq!(interp.runtime_resource_gate_count(), 1);

    sema_mcp::close_handle(&handle);

    assert_eq!(
        interp.runtime_resource_gate_count(),
        0,
        "the host-only close capability must remove the live runtime gate"
    );
    let error = interp
        .eval_str_via_runtime("(mcp/tools server)")
        .expect_err("close_handle removes the connection registry entry");
    assert!(error.to_string().contains("not registered"), "{error}");
}

#[test]
fn foreign_runtime_mcp_close_is_offloaded_and_closes_owner_gate() {
    let markers = Markers::new("foreign-runtime-close");
    let (_server, port) = start_delayed_http_server(&markers);
    let release = sema_str(&markers.release.to_string_lossy());
    let owner = mcp_eval_interpreter();
    let caller = mcp_eval_interpreter();
    owner
        .eval_str_via_runtime(&format!(
            "(define server (mcp/connect {{:url \"http://127.0.0.1:{port}/mcp\"}}))"
        ))
        .expect("owner connects to delayed close server");
    owner
        .eval_str_via_runtime("(mcp/tools server)")
        .expect("owner lazily creates the connection gate");
    let handle = owner
        .eval_str_via_runtime("server")
        .expect("read opaque MCP handle")
        .as_str()
        .expect("MCP handle is a string")
        .to_string();
    let handle = sema_str(&handle);
    assert_eq!(owner.runtime_resource_gate_count(), 1);
    assert_eq!(caller.runtime_resource_gate_count(), 0);

    let result = caller
        .eval_str_via_runtime(&format!(
            r#"
            (let ((operation (async/spawn (fn () (mcp/close {handle}))))
                  (sibling (async/spawn (fn ()
                    (file/write {release} "release")
                    :sibling))))
              (list (async/await operation) (async/await sibling)))
            "#
        ))
        .expect("foreign MCP close uses a caller-runtime External offload");
    let values = result.as_seq().expect("close result and sibling marker");
    assert!(values[0].is_nil());
    assert_eq!(values[1].as_keyword().as_deref(), Some("sibling"));
    assert!(markers.entered.exists(), "server observed protocol close");
    assert!(
        !markers.timed_out.exists(),
        "foreign close blocked the caller VM until the server fallback fired"
    );
    assert_eq!(owner.runtime_resource_gate_count(), 0);
    assert_eq!(caller.runtime_resource_gate_count(), 0);
}

#[test]
fn runtime_generated_mcp_handler_wait_allows_sibling_progress() {
    let markers = Markers::new("generated-handler-progress");
    let config = delayed_stdio_config("call", &markers);
    let release = sema_str(&markers.release.to_string_lossy());
    let interp = Interpreter::new();
    interp
        .eval_str_via_runtime(&format!("(define server (mcp/connect {config}))"))
        .expect("connect delayed generated-handler server");
    install_generated_echo_handler(&interp, "generated-echo");

    let result = interp
        .eval_str_via_runtime(&format!(
            r#"
            (let ((operation (async/spawn (fn () (generated-echo "hello"))))
                  (sibling (async/spawn (fn ()
                    (file/write {release} "release")
                    :sibling))))
              (list (async/await operation) (async/await sibling)))
            "#
        ))
        .expect("generated MCP handler and its sibling complete");
    let values = result
        .as_seq()
        .expect("generated handler result and sibling marker");
    assert_eq!(values[0].as_str(), Some("hello"));
    assert_eq!(values[1].as_keyword().as_deref(), Some("sibling"));
    assert!(markers.entered.exists(), "server delayed tools/call");
    assert!(
        !markers.timed_out.exists(),
        "generated handler blocked the VM until the server's fallback fired"
    );
    interp.eval_str_via_runtime("(mcp/close server)").ok();
}

#[test]
fn runtime_mcp_close_wait_allows_sibling_progress() {
    let markers = Markers::new("close-progress");
    let (_server, port) = start_delayed_http_server(&markers);
    let release = sema_str(&markers.release.to_string_lossy());
    let interp = Interpreter::new();
    interp
        .eval_str_via_runtime(&format!(
            "(define server (mcp/connect {{:url \"http://127.0.0.1:{port}/mcp\"}}))"
        ))
        .expect("connect delayed close server");
    let result = interp
        .eval_str_via_runtime(&format!(
            r#"
            (let ((operation (async/spawn (fn () (mcp/close server))))
                  (sibling (async/spawn (fn ()
                    (file/write {release} "release")
                    :sibling))))
              (list (async/await operation) (async/await sibling)))
            "#
        ))
        .expect("mcp/close and its sibling complete");
    let values = result.as_seq().expect("close result and sibling marker");
    assert!(values[0].is_nil(), "mcp/close returns nil");
    assert_eq!(values[1].as_keyword().as_deref(), Some("sibling"));
    assert!(markers.entered.exists(), "server delayed DELETE");
    assert!(
        !markers.timed_out.exists(),
        "mcp/close blocked the VM until the server's fallback fired"
    );
}

#[test]
fn runtime_mcp_connect_wait_is_promptly_cancellable() {
    let markers = Markers::new("connect-cancel");
    let config = delayed_stdio_config("connect", &markers);
    let interp = mcp_eval_interpreter();
    assert_cancelled_before_server_fallback(
        &interp,
        &format!("(mcp/connect {config})"),
        &markers,
        CancellationResourceOracle::StdioServerExit,
    );
}

#[test]
fn runtime_mcp_tools_wait_is_promptly_cancellable() {
    let markers = Markers::new("tools-cancel");
    let config = delayed_stdio_config("tools", &markers);
    let interp = mcp_eval_interpreter();
    interp
        .eval_str_via_runtime(&format!("(define server (mcp/connect {config}))"))
        .expect("connect delayed tools server");
    assert_cancelled_before_server_fallback(
        &interp,
        "(mcp/tools server)",
        &markers,
        CancellationResourceOracle::StdioServerExit,
    );
    assert_connection_tombstoned(&interp, "cancelled during mcp/tools");
}

#[test]
fn runtime_generated_mcp_handler_wait_is_promptly_cancellable() {
    let markers = Markers::new("generated-handler-cancel");
    let config = delayed_stdio_config("call", &markers);
    let interp = mcp_eval_interpreter();
    interp
        .eval_str_via_runtime(&format!("(define server (mcp/connect {config}))"))
        .expect("connect delayed generated-handler server");
    install_generated_echo_handler_for_eval(&interp, "generated-echo");

    assert_cancelled_before_server_fallback(
        &interp,
        "(generated-echo \"hello\")",
        &markers,
        CancellationResourceOracle::StdioServerExit,
    );
    assert_connection_tombstoned(&interp, "cancelled mid-call");
}

#[test]
fn runtime_mcp_close_wait_is_promptly_cancellable() {
    let markers = Markers::new("close-cancel");
    let (_server, port) = start_delayed_http_server(&markers);
    let interp = mcp_eval_interpreter();
    interp
        .eval_str_via_runtime(&format!(
            "(define server (mcp/connect {{:url \"http://127.0.0.1:{port}/mcp\"}}))"
        ))
        .expect("connect delayed close server");
    assert_cancelled_before_server_fallback(
        &interp,
        "(mcp/close server)",
        &markers,
        CancellationResourceOracle::HttpPeerDisconnect,
    );
}

#[test]
fn runtime_mcp_tools_to_sema_matches_synchronous_shape() {
    let markers = Markers::new("tools-to-sema-parity");
    let config = delayed_stdio_config("immediate", &markers);
    let interp = Interpreter::new();
    interp
        .eval_str(&format!("(define server (mcp/connect {config}))"))
        .expect("connect parity server");

    let synchronous = interp
        .eval_str("(mcp/tools->sema server)")
        .expect("synchronous tools->sema");
    let runtime = interp
        .eval_str_via_runtime("(mcp/tools->sema server)")
        .expect("runtime tools->sema");
    let synchronous = synchronous.as_seq().expect("synchronous tool defs");
    let runtime = runtime.as_seq().expect("runtime tool defs");
    assert_eq!(synchronous.len(), runtime.len());
    for (sync_value, runtime_value) in synchronous.iter().zip(runtime) {
        let sync_tool = sync_value.as_tool_def_rc().expect("synchronous tool def");
        let runtime_tool = runtime_value.as_tool_def_rc().expect("runtime tool def");
        assert_eq!(sync_tool.name, runtime_tool.name);
        assert_eq!(sync_tool.description, runtime_tool.description);
        assert_eq!(sync_tool.parameters, runtime_tool.parameters);
    }

    interp.eval_str("(mcp/close server)").ok();
}

fn assert_sync_runtime_error_parity(source: &str) {
    let synchronous = Interpreter::new()
        .eval_str(source)
        .expect_err("synchronous MCP operation must fail");
    let runtime = Interpreter::new()
        .eval_str_via_runtime(source)
        .expect_err("runtime MCP operation must fail");
    assert_error_parity(&synchronous, &runtime);
}

fn assert_error_parity(synchronous: &sema_core::SemaError, runtime: &sema_core::SemaError) {
    assert_eq!(synchronous.to_string(), runtime.to_string());
    assert_eq!(synchronous.hint(), runtime.hint());
    assert_eq!(synchronous.note(), runtime.note());
}

#[test]
fn runtime_mcp_tools_decoder_errors_match_synchronous_errors() {
    let markers = Markers::new("tools-error-parity");
    let config = delayed_stdio_config("tools-error", &markers);
    let interp = Interpreter::new();
    interp
        .eval_str_via_runtime(&format!("(define server (mcp/connect {config}))"))
        .expect("connect tools-error server");

    for source in ["(mcp/tools server)", "(mcp/tools->sema server)"] {
        let synchronous = interp
            .eval_str(source)
            .expect_err("synchronous tools/list must surface the server error");
        let runtime = interp
            .eval_str_via_runtime(source)
            .expect_err("runtime tools/list decoder must surface the server error");
        assert!(
            runtime
                .to_string()
                .contains("MCP RPC error -32042: tools exploded"),
            "error must originate after server dispatch: {runtime}"
        );
        assert_error_parity(&synchronous, &runtime);
    }

    interp.eval_str_via_runtime("(mcp/close server)").ok();
}

#[test]
fn runtime_mcp_operation_errors_match_synchronous_errors() {
    assert_sync_runtime_error_parity(
        r#"(mcp/connect {:command "definitely-not-a-real-mcp-command"})"#,
    );
    assert_sync_runtime_error_parity(r#"(mcp/tools "mcp-not-registered")"#);
    assert_sync_runtime_error_parity(r#"(mcp/tools->sema "mcp-not-registered")"#);
    assert_sync_runtime_error_parity(r#"(mcp/close "mcp-not-registered")"#);
}
