//! Acceptance gate for MCP-4 / issue #96: the MCP client builtins
//! (`mcp/connect`, `mcp/call`, `mcp/tools`, `mcp/close`) must offload under the
//! cooperative scheduler instead of blocking the whole VM thread.
//!
//! Every mock server here is a small stdio JSON-RPC Python script (matching
//! `mcp_builtin_test.rs`/`mcp_cassette_test.rs`'s harness), extended with
//! deterministic coordination (marker files, a busy flag) so the assertions
//! below are ordering/completion signals — never wall-clock timing thresholds.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use sema::{Interpreter, Value};
use sema_llm::builtins::{install_cassette, take_cassette};
use sema_llm::cassette::{Cassette, CassetteMode};
use sema_mcp::{connect_from_config, ConnectOpts};

/// A unique path under the system temp dir for one test's scratch file(s).
fn unique_temp_path(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "sema-mcp-async-{tag}-{}-{n}.marker",
        std::process::id()
    ))
}

/// Sema string-literal-encode a Rust string (for interpolating into a
/// generated Sema program, e.g. a marker file path).
fn sema_str(s: &str) -> String {
    let encoded = serde_json::to_string(s).expect("string encodes to JSON");
    // JSON string syntax is a valid Sema string literal.
    encoded
}

// ── Scenario 1: cross-connection overlap ────────────────────────────────────
//
// Server A withholds its `tools/call` response until it observes a marker
// file that server B's handler touches — a one-directional signal proving B
// received (and answered) its own request while A's was still in flight. Two
// `async/spawn`ed calls, one to each server, both completing proves the
// overlap; a serialized/blocking implementation deadlocks (A waits forever
// for a marker B's task never gets a chance to write) and the surrounding
// `async/timeout` turns that into a clean test failure instead of a hang.

const SERVER_WITHHOLDS_UNTIL_MARKER: &str = r#"
import json, sys, os, time
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
initialized = False
marker = sys.argv[1]
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    r = json.loads(line); method = r.get("method"); rid = r.get("id")
    if rid is None:
        if method == "notifications/initialized":
            initialized = True
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
initialized = False
marker = sys.argv[1]
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    r = json.loads(line); method = r.get("method"); rid = r.get("id")
    if rid is None:
        if method == "notifications/initialized":
            initialized = True
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

#[test]
fn cross_connection_overlap_proves_no_serialization() {
    let marker = unique_temp_path("overlap");
    let _ = std::fs::remove_file(&marker);
    let marker_arg = sema_str(&marker.to_string_lossy());

    let interp = Interpreter::new();
    let a_encoded = sema_str(SERVER_WITHHOLDS_UNTIL_MARKER);
    let b_encoded = sema_str(SERVER_TOUCHES_MARKER);
    interp
        .eval_str(&format!(
            r#"(define a (mcp/connect {{:command "python3" :args ["-c" {a_encoded} {marker_arg}]}}))"#
        ))
        .expect("connect a");
    interp
        .eval_str(&format!(
            r#"(define b (mcp/connect {{:command "python3" :args ["-c" {b_encoded} {marker_arg}]}}))"#
        ))
        .expect("connect b");

    let program = r#"
        (async/timeout 15000
          (async/spawn (fn ()
            (async/all
              (list
                (async/spawn (fn () (mcp/call a "wait_for_marker" {})))
                (async/spawn (fn () (mcp/call b "touch_marker" {}))))))))
    "#;
    let result = interp
        .eval_str(program)
        .expect("both connections' calls complete without deadlock");
    let items = result.as_seq().expect("async/all returns a list");
    assert_eq!(items.len(), 2);
    let texts: Vec<&str> = items.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        texts.contains(&"a-saw-marker"),
        "server A must have observed B's marker before answering (proves in-flight overlap); got {texts:?}"
    );
    assert!(texts.contains(&"b-done"), "got {texts:?}");

    interp.eval_str("(mcp/close a)").ok();
    interp.eval_str("(mcp/close b)").ok();
    let _ = std::fs::remove_file(&marker);
}

// ── Scenario 2: scheduler not stalled by a slow call ────────────────────────
//
// One task makes a `mcp/call` to a server that sleeps briefly before
// answering; a sibling task does no I/O at all. A completion-order assertion
// via a channel (not a sleep) proves the sibling finished BEFORE the slow
// call resolved — impossible if the call blocked the VM thread.

const SLOW_SERVER: &str = r#"
import json, sys, time
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
initialized = False
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    r = json.loads(line); method = r.get("method"); rid = r.get("id")
    if rid is None:
        if method == "notifications/initialized":
            initialized = True
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "serverInfo": {"name": "slow", "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
            {"name": "slow", "description": "slow",
             "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        time.sleep(0.4)
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "content": [{"type": "text", "text": "slow-done"}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "no"}})
"#;

#[test]
fn scheduler_not_stalled_sibling_completes_before_slow_call() {
    let interp = Interpreter::new();
    let encoded = sema_str(SLOW_SERVER);
    interp
        .eval_str(&format!(
            r#"(define server (mcp/connect {{:command "python3" :args ["-c" {encoded}]}}))"#
        ))
        .expect("connect");

    let program = r#"
        (let ((order (channel/new 2)))
          (async/all
            (list
              (async/spawn (fn ()
                (mcp/call server "slow" {})
                (channel/send order "slow-call")))
              (async/spawn (fn ()
                (channel/send order "sibling")))))
          (list (channel/recv order) (channel/recv order)))
    "#;
    let result = interp
        .eval_str(program)
        .expect("both tasks complete without stalling the scheduler");
    let items = result.as_seq().expect("a list of two order markers");
    let order: Vec<&str> = items.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(
        order,
        vec!["sibling", "slow-call"],
        "the no-I/O sibling must finish and record its marker BEFORE the \
         slow mcp/call resolves — a blocking implementation would record \
         [slow-call, sibling] instead (got {order:?})"
    );

    interp.eval_str("(mcp/close server)").ok();
}

// ── Shared: a server with a per-connection busy flag + incrementing counter ─
//
// Used by scenarios 3 (same-handle queueing) and 4 (queue wakeup). A second
// `tools/call` arriving before the first has been answered is a hard
// server-side error — proof the client serialized requests on this ONE
// connection, exactly as the MCP wire protocol requires.

const BUSY_COUNTER_SERVER: &str = r#"
import json, sys, time
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
initialized = False
busy = False
counter = 0
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    r = json.loads(line); method = r.get("method"); rid = r.get("id")
    if rid is None:
        if method == "notifications/initialized":
            initialized = True
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

fn connect_busy_counter_expr() -> String {
    let encoded = sema_str(BUSY_COUNTER_SERVER);
    format!(r#"(define server (mcp/connect {{:command "python3" :args ["-c" {encoded}]}}))"#)
}

// ── Scenario 3: same-handle queueing ────────────────────────────────────────

#[test]
fn same_handle_queueing_serializes_two_concurrent_calls() {
    let interp = Interpreter::new();
    interp
        .eval_str(&connect_busy_counter_expr())
        .expect("connect");

    let program = r#"
        (async/all
          (list
            (async/spawn (fn () (mcp/call server "count" {})))
            (async/spawn (fn () (mcp/call server "count" {})))))
    "#;
    let result = interp.eval_str(program).expect(
        "both queued calls must succeed — a server-side overlap error means the \
                  client sent a second request before the first was answered",
    );
    let items = result.as_seq().expect("a list of two results");
    let mut vals: Vec<&str> = items.iter().map(|v| v.as_str().unwrap()).collect();
    vals.sort_unstable();
    assert_eq!(vals, vec!["call-1", "call-2"]);

    interp.eval_str("(mcp/close server)").ok();
}

// ── Scenario 4: queue wakeup (lost-wakeup regression) ───────────────────────

#[test]
fn queue_wakeup_five_queued_calls_all_complete() {
    let interp = Interpreter::new();
    interp
        .eval_str(&connect_busy_counter_expr())
        .expect("connect");

    // A generous but bounded timeout: this is a deadlock-prevention gate (all
    // N queued calls must eventually complete), not a timing assertion — see
    // the module doc. If the check-in `notify_io_complete()` were dropped, the
    // scheduler's bounded `io_park` fallback still recovers (just slower);
    // this test's job is to catch a genuine stuck-forever regression in the
    // Acquire-phase requeue loop.
    let program = r#"
        (async/timeout 20000
          (async/spawn (fn ()
            (async/all
              (list
                (async/spawn (fn () (mcp/call server "count" {})))
                (async/spawn (fn () (mcp/call server "count" {})))
                (async/spawn (fn () (mcp/call server "count" {})))
                (async/spawn (fn () (mcp/call server "count" {})))
                (async/spawn (fn () (mcp/call server "count" {}))))))))
    "#;
    let result = interp
        .eval_str(program)
        .expect("all five queued calls must complete, not park forever");
    let items = result.as_seq().expect("a list of five results");
    assert_eq!(items.len(), 5);
    let mut vals: Vec<&str> = items.iter().map(|v| v.as_str().unwrap()).collect();
    vals.sort_unstable();
    assert_eq!(
        vals,
        vec!["call-1", "call-2", "call-3", "call-4", "call-5"],
        "every queued call must have run exactly once, strictly sequentially"
    );

    interp.eval_str("(mcp/close server)").ok();
}

// ── Scenario 5: cancellation tombstones the connection ──────────────────────
//
// The mock server sleeps a few real seconds before answering (long enough
// that explicit cancellation always stops the awaiting task first, short enough the
// orphaned child process exits on its own soon after — no indefinite zombie).

const SLEEPY_SERVER: &str = r#"
import json, sys, time
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
initialized = False
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    r = json.loads(line); method = r.get("method"); rid = r.get("id")
    if rid is None:
        if method == "notifications/initialized":
            initialized = True
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "serverInfo": {"name": "sleepy", "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
            {"name": "hang", "description": "never answers promptly",
             "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        time.sleep(3)
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "content": [{"type": "text", "text": "too-late"}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "no"}})
"#;

#[test]
fn cancellation_tombstones_connection_and_interpreter_stays_healthy() {
    let interp = Interpreter::new();
    let encoded = sema_str(SLEEPY_SERVER);
    interp
        .eval_str(&format!(
            r#"(define h (mcp/connect {{:command "python3" :args ["-c" {encoded}]}}))"#
        ))
        .expect("connect");

    let program = r#"
        (define p (async/spawn (fn () (mcp/call h "hang" {}))))
        (async/spawn (fn () (async/sleep 200) (async/cancel p)))
        (try (async/await p) (catch e :caught))
    "#;
    let result = interp
        .eval_str(program)
        .expect("explicitly cancelled mcp/call evaluated");
    assert_eq!(
        result,
        Value::keyword("caught"),
        "explicit cancellation must stop the slow call"
    );

    // A follow-up call on the now-tombstoned handle must fail fast with the
    // documented reason + reconnect hint — never hang.
    let err = interp
        .eval_str(r#"(mcp/call h "hang" {})"#)
        .expect_err("a tombstoned handle must error, not hang or silently succeed");
    let msg = err.to_string();
    assert!(
        msg.contains("connection lost") && msg.contains("cancelled mid-call"),
        "expected the documented tombstone message, got: {msg}"
    );

    // The interpreter/run remains healthy: ordinary evaluation still works
    // and no task is left orphaned in the scheduler.
    let healthy = interp
        .eval_str("(+ 1 2)")
        .expect("interpreter must remain usable after the cancellation");
    assert_eq!(healthy, Value::int(3));
    assert_eq!(
        sema_vm::scheduler_task_count(),
        0,
        "the cancelled task must be reaped, not left orphaned in the scheduler"
    );
}

// ── Scenario 6: sync (non-async) context is unchanged ───────────────────────

#[test]
fn sync_context_mcp_call_is_unaffected_by_the_async_offload() {
    let interp = Interpreter::new();
    interp
        .eval_str(&connect_busy_counter_expr())
        .expect("connect");

    // A plain top-level (non-async-context) call: fully synchronous, exactly
    // as before this change — no scheduler, no yield, no offload.
    let result = interp
        .eval_str(r#"(mcp/call server "count" {})"#)
        .expect("sync mcp/call");
    assert_eq!(result.as_str(), Some("call-1"));

    interp.eval_str("(mcp/close server)").ok();
}

// ── Scenario 7: cassette replay stays synchronous in async context ─────────

const REPLAY_COUNTER_SERVER: &str = r#"
import json, sys
initialized = False
counter = 0
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    r = json.loads(line); method = r.get("method"); rid = r.get("id")
    if rid is None:
        if method == "notifications/initialized":
            initialized = True
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "serverInfo": {"name": "replay-counter", "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
            {"name": "count", "description": "increment",
             "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        counter += 1
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "content": [{"type": "text", "text": "call-%d" % counter}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "no"}})
"#;

fn tape_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "sema-mcp-async-cassette-{}-{}/tape.ndjson",
        std::process::id(),
        n
    ))
}

#[test]
fn cassette_replay_stays_synchronous_inside_async_task() {
    let tape = tape_path();
    let interp = Interpreter::new();
    let encoded = sema_str(REPLAY_COUNTER_SERVER);
    interp
        .eval_str(&format!(
            r#"(define server (mcp/connect {{:command "python3" :args ["-c" {encoded}]}}))"#
        ))
        .expect("connect");

    // --- Record (sync, top level): the real call runs and is taped. ---
    install_cassette(Cassette::load(tape.clone(), CassetteMode::Record));
    let r1 = interp
        .eval_str(r#"(mcp/call server "count" {})"#)
        .expect("record call");
    assert_eq!(r1.as_str(), Some("call-1"));
    take_cassette()
        .expect("cassette installed")
        .save()
        .expect("save tape");

    // --- Replay INSIDE an async task: must resolve without ever offloading
    //     (no live server touch — the counter must NOT advance). ---
    install_cassette(Cassette::load(tape, CassetteMode::Replay));
    let r2 = interp
        .eval_str(r#"(await (async/spawn (fn () (mcp/call server "count" {}))))"#)
        .expect("replay call inside async task");
    assert_eq!(
        r2.as_str(),
        Some("call-1"),
        "replay in async context must return the recorded value, not re-hit the server"
    );

    // --- Proof: drop the cassette and call for real → the server advances to
    //     call-2, confirming the async replay above did NOT touch it. ---
    take_cassette();
    let r3 = interp
        .eval_str(r#"(mcp/call server "count" {})"#)
        .expect("live call");
    assert_eq!(r3.as_str(), Some("call-2"));

    interp.eval_str("(mcp/close server)").ok();
}

// ── Scenario 8: mid-call 401 → non-interactive reauth fails cleanly, async ──
//
// The one new thread-hop-sensitive path this offload introduced that scenarios
// 1-7 don't cover: `call_tool_async`'s mid-session 401-then-reauth-then-retry
// branch, exercised from INSIDE `async/spawn` (so the offloaded call path with
// `OpenerSource::Resolved` runs, never `OpenerSource::Live`).
//
// The FULL success slice (401 → silent refresh-token self-heal → retry
// succeeds) is not reachable here without adding a new production seam:
// `reauthorize_async` hardcodes `crate::oauth::store::default_store()` with no
// injection point for a test-owned token store, and there is no stored
// refresh token for this test's freshly-bound loopback URL to refresh from.
// Mutating process-wide env (`HOME`/`XDG_CONFIG_HOME`) to redirect the default
// store path would also race every other test in this shared-process binary —
// exactly the "new production seam" / unsafe-global-mutation the task brief
// says not to introduce. So this test exercises the closest deterministic,
// hermetic slice instead: `connect_from_config` with `interactive_auth: false`
// (the same `ConnectOpts` a workflow `:mcp` manifest uses) means a mid-call 401
// drives `NoInteractiveDriver`, which refuses the browser leg unconditionally —
// no network, no thread hop, no flakiness — so the original 401 must surface
// cleanly rather than hang or panic, and the scheduler must not stall a
// sibling task while the multi-round-trip reauth attempt (discovery + DCR) is
// in flight on the `sema-io` pool.
//
// The mock plays MCP server AND (partial) OAuth authorization server: the
// first `tools/call` for tool `flaky` always 401s with a `WWW-Authenticate`
// challenge; `/.well-known/oauth-protected-resource`,
// `/.well-known/oauth-authorization-server`, and `/register` (DCR) all answer
// so `reauth_on_challenge`'s `login()` reaches the interactive leg — the two
// marker files below are touched by discovery/DCR, proving reauth was actually
// attempted (not silently skipped) before `NoInteractiveDriver::drive()`
// refuses it. No `/authorize`/`/token` handler is needed: `drive()` never
// makes a network call.

const FLAKY_AUTH_HTTP_SERVER: &str = r#"
import json, sys, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

PORT = None
MARKER_DIR = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def base(self):
        return "http://127.0.0.1:%d" % PORT

    def _json(self, obj, code=200, headers=None):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        p = urlparse(self.path)
        if p.path == "/.well-known/oauth-protected-resource":
            open(os.path.join(MARKER_DIR, "discovery-hit"), "w").close()
            return self._json({"resource": self.base() + "/mcp",
                               "authorization_servers": [self.base()],
                               "scopes_supported": ["mcp:tools"]})
        if p.path == "/.well-known/oauth-authorization-server":
            return self._json({"issuer": self.base(),
                               "authorization_endpoint": self.base() + "/authorize",
                               "token_endpoint": self.base() + "/token",
                               "registration_endpoint": self.base() + "/register",
                               "code_challenge_methods_supported": ["S256"]})
        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        p = urlparse(self.path)
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b""
        if p.path == "/register":
            open(os.path.join(MARKER_DIR, "register-hit"), "w").close()
            return self._json({"client_id": "reauth-test-client"}, code=201)
        if p.path == "/mcp":
            msg = json.loads(raw) if raw else {}
            method = msg.get("method")
            rid = msg.get("id")
            if rid is None:
                self.send_response(202)
                self.end_headers()
                return
            if method == "initialize":
                return self._json({"jsonrpc": "2.0", "id": rid, "result": {
                    "protocolVersion": "2025-11-25", "capabilities": {},
                    "serverInfo": {"name": "flaky-auth", "version": "1.0"}}},
                    headers={"Mcp-Session-Id": "sess-1"})
            if method == "tools/list":
                return self._json({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
                    {"name": "flaky", "description": "always needs auth mid-session",
                     "inputSchema": {"type": "object", "properties": {}}}]}})
            if method == "tools/call":
                self.send_response(401)
                self.send_header("WWW-Authenticate",
                                 'Bearer error="invalid_token", resource_metadata="%s/.well-known/oauth-protected-resource"' % self.base())
                self.end_headers()
                return
            return self._json({"jsonrpc": "2.0", "id": rid,
                               "error": {"code": -32601, "message": "Method not found"}})
        self.send_response(404)
        self.end_headers()

srv = HTTPServer(("127.0.0.1", 0), H)
PORT = srv.server_address[1]
print(PORT, flush=True)
srv.serve_forever()
"#;

struct ServerGuard {
    child: Child,
    _stdout: BufReader<ChildStdout>,
}
impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_flaky_auth_server(marker_dir: &Path) -> (ServerGuard, u16) {
    let mut child = Command::new("python3")
        .args(["-c", FLAKY_AUTH_HTTP_SERVER, &marker_dir.to_string_lossy()])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn python3 flaky-auth http server");
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read port");
    let port: u16 = line.trim().parse().expect("port");
    (
        ServerGuard {
            child,
            _stdout: reader,
        },
        port,
    )
}

fn http_config(url: &str) -> Value {
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("url"), Value::string(url));
    Value::map(map)
}

fn unique_marker_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("sema-mcp-async-{tag}-{}-{n}", std::process::id()))
}

#[test]
fn async_context_mid_call_401_noninteractive_reauth_fails_cleanly() {
    let marker_dir = unique_marker_dir("reauth-markers");
    std::fs::create_dir_all(&marker_dir).expect("create marker dir");

    let (_server, port) = start_flaky_auth_server(&marker_dir);
    let url = format!("http://127.0.0.1:{port}/mcp");

    // Plain synchronous connect (never inside async/spawn — same as the
    // workflow `:mcp` pre-phase): `initialize`/`tools/list` never 401, so this
    // succeeds without any OAuth involvement.
    let opts = ConnectOpts {
        interactive_auth: false,
        allowed_tools: None,
    };
    let handle_value =
        connect_from_config(&http_config(&url), opts).expect("initial connect must succeed");
    let handle = handle_value
        .as_str()
        .expect("handle is a string")
        .to_string();
    let handle_lit = sema_str(&handle);

    // `mcp/call`'s offloaded failure rejects the WHOLE spawned task at the
    // scheduler level (`YieldReason::AwaitIo` resolving to `Err` transitions
    // that task straight to `Failed` — see `wake_blocked_tasks` in
    // `scheduler.rs` — it never resumes the task's own Sema code with a
    // catchable in-task exception). So `try`/`catch` must wrap the COMBINATOR
    // awaiting that task's promise (`async/all`, same idiom as the existing
    // `cancellation_tombstones_connection...` scenario's `(try (async/await p)
    // (catch e ...))`), not the `mcp/call` expression itself — a `try`
    // placed directly around `mcp/call` inside the failing task would never
    // run its `catch` arm.
    let interp = Interpreter::new();
    let program = format!(
        r#"
        (async/timeout 15000
          (async/spawn (fn ()
            (let ((order (channel/new 2)))
              (try
                (async/all
                  (list
                    (async/spawn (fn () (mcp/call {handle_lit} "flaky" {{}})))
                    (async/spawn (fn () (channel/send order "sibling")))))
                (catch e (channel/send order (:message e))))
              (list (channel/recv order) (channel/recv order))))))
        "#
    );
    let result = interp
        .eval_str(&program)
        .expect("the caught reauth failure must not hang/panic the whole program");
    let items = result.as_seq().expect("a list of two order markers");
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0].as_str(),
        Some("sibling"),
        "the no-I/O sibling task must finish and record its marker BEFORE the \
         401-then-reauth-then-refuse round trip resolves — a stalled scheduler \
         would reorder this (got {items:?})"
    );
    let outcome = items[1]
        .as_str()
        .expect("second item is the caught error's :message string");
    assert!(
        outcome.contains("HTTP 401") && outcome.contains("tools/call"),
        "a failed non-interactive reauth must surface the ORIGINAL 401 error \
         (call_tool_async's fallback on reauth failure), not a different \
         message; got: {outcome}"
    );

    // Reauth was actually ATTEMPTED (not skipped): discovery and DCR both ran
    // before `NoInteractiveDriver::drive()` cleanly refused the browser leg.
    assert!(
        marker_dir.join("discovery-hit").exists(),
        "the 401 branch must have driven OAuth discovery, proving the \
         reauth-then-retry path actually ran"
    );
    assert!(
        marker_dir.join("register-hit").exists(),
        "the 401 branch must have reached dynamic client registration before \
         the non-interactive driver refuses the browser leg"
    );

    // The interpreter/scheduler remains healthy: no orphaned task, ordinary
    // evaluation still works.
    let healthy = interp
        .eval_str("(+ 1 2)")
        .expect("interpreter must remain usable after the failed reauth attempt");
    assert_eq!(healthy, Value::int(3));
    assert_eq!(
        sema_vm::scheduler_task_count(),
        0,
        "no task should be left orphaned in the scheduler after the offloaded \
         401-reauth-refuse-retry sequence completes"
    );

    sema_mcp::close_handle(&handle_value);
    let _ = std::fs::remove_dir_all(&marker_dir);
}
