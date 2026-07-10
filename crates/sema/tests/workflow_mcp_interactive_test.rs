//! End-to-end tests for INTERACTIVE MCP auth inline on `sema workflow run`
//! (docs/plans/2026-06-24-workflow-mcp-auth.md §3, "run-start interactive
//! login"): with `sema::workflow_mcp::set_interactive_auth(true)` forced (the
//! same seam `run_workflow_command` flips on a real TTY) and an injectable
//! opener wired via `sema::workflow_mcp::set_interactive_login_opener`, a run
//! with no stored credentials performs the SAME browser/loopback OAuth flow
//! `sema mcp login` runs, inline, and continues — instead of gating with
//! `{:status :needs-auth}`.
//!
//! The mock server duplicates `workflow_mcp_e2e_test.rs`'s harness (same
//! reasoning as `workflow_mcp_cli_e2e_test.rs`'s doc comment: fixtures are
//! cheaper to duplicate than to couple across files) plus an `/authorize` GET
//! and an `authorization_code` grant on `/token`, so a full login can run
//! against it — `workflow_mcp_e2e_test.rs`'s server never needed those (every
//! one of its scenarios starts from a pre-seeded or absent token, never a live
//! consent). A `DENY` toggle on `/authorize` simulates a declined consent for
//! the fallback-proof test, without waiting out the real login timeout.
//!
//! Env-var discipline: identical to `workflow_mcp_e2e_test.rs` — every
//! `SEMA_WORKFLOW_*`/`SEMA_MCP_AUTH_KEY` var is process-global, so every test
//! funnels through [`run_workflow`], which holds a process-wide mutex for its
//! whole set/run/clear window. The interactive-auth thread-locals
//! (`set_interactive_auth`/`set_interactive_login_opener`) are reset inside
//! the SAME window for the same reason cargo's test-thread pool can reuse an
//! OS thread across tests in this binary — a thread-local is otherwise
//! per-OS-thread, not per-test.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use sema::InterpreterBuilder;
use sema_core::{Caps, Sandbox};
use sema_mcp::oauth::scoped::ScopedFileStore;
use sema_mcp::oauth::store::{ClientInfo, StoredCredentials, TokenSet, TokenStore};

static SERIAL: Mutex<()> = Mutex::new(());

/// A fixed 32-byte key, as 64 hex chars — a TEST key, never used for anything
/// real; each test's run dir is thrown away afterward.
fn auth_key_hex() -> String {
    "33".repeat(32)
}

fn auth_key_bytes() -> [u8; 32] {
    let hex = auth_key_hex();
    let mut k = [0u8; 32];
    for (i, byte) in k.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    k
}

/// The mock server: `workflow_mcp_e2e_test.rs`'s discovery/`/mcp` handler,
/// plus `/authorize` (redirects to the loopback callback with either a code or
/// `error=access_denied`, per `{deny}`) and `/token` accepting the
/// `authorization_code` grant (in addition to `refresh_token`, kept for
/// parity even though these tests don't exercise it). No DCR
/// `registration_endpoint` — the test workflow supplies `:client-id` directly.
fn server_script(deny: bool) -> String {
    let deny_py = if deny { "True" } else { "False" };
    format!(
        r#"
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs, urlencode

PORT = None
DENY = {deny_py}
RECOGNIZED = {{"consented-token-xyz"}}

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
        for k, v in (headers or {{}}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        p = urlparse(self.path)
        if p.path == "/.well-known/oauth-protected-resource":
            return self._json({{"resource": self.base() + "/mcp",
                               "authorization_servers": [self.base()],
                               "scopes_supported": ["mcp:tools"]}})
        if p.path == "/.well-known/oauth-authorization-server":
            return self._json({{"issuer": self.base(),
                               "authorization_endpoint": self.base() + "/authorize",
                               "token_endpoint": self.base() + "/token",
                               "code_challenge_methods_supported": ["S256"]}})
        if p.path == "/authorize":
            q = parse_qs(p.query)
            redirect_uri = q.get("redirect_uri", [""])[0]
            state = q.get("state", [""])[0]
            if DENY:
                loc = redirect_uri + "?" + urlencode({{"error": "access_denied",
                                                       "error_description": "user declined",
                                                       "state": state}})
            else:
                loc = redirect_uri + "?" + urlencode({{"code": "authcode-interactive-1",
                                                       "state": state}})
            self.send_response(302)
            self.send_header("Location", loc)
            self.end_headers()
            return
        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        p = urlparse(self.path)
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b""
        if p.path == "/token":
            body = parse_qs(raw.decode())
            grant = body.get("grant_type", [""])[0]
            if grant == "authorization_code":
                return self._json({{"access_token": "consented-token-xyz",
                                   "refresh_token": "consented-refresh-xyz",
                                   "token_type": "Bearer", "expires_in": 3600,
                                   "scope": "mcp:tools"}})
            if grant == "refresh_token":
                return self._json({{"access_token": "consented-token-xyz",
                                   "refresh_token": "consented-refresh-xyz",
                                   "token_type": "Bearer", "expires_in": 3600,
                                   "scope": "mcp:tools"}})
            return self._json({{"error": "unsupported_grant_type"}}, code=400)
        if p.path != "/mcp":
            self.send_response(404)
            self.end_headers()
            return
        auth = self.headers.get("Authorization", "")
        token = auth[len("Bearer "):] if auth.startswith("Bearer ") else ""
        if token not in RECOGNIZED:
            self.send_response(401)
            self.send_header(
                "WWW-Authenticate",
                'Bearer resource_metadata="%s/.well-known/oauth-protected-resource"' % self.base(),
            )
            self.end_headers()
            return
        msg = json.loads(raw) if raw else {{}}
        method = msg.get("method")
        rid = msg.get("id")
        if rid is None:
            self.send_response(202)
            self.end_headers()
            return
        if method == "initialize":
            return self._json(
                {{"jsonrpc": "2.0", "id": rid, "result": {{
                    "protocolVersion": "2025-11-25", "capabilities": {{}},
                    "serverInfo": {{"name": "interactive-e2e-server", "version": "1.0"}}}}}},
                headers={{"Mcp-Session-Id": "sess-interactive-e2e"}},
            )
        if method == "tools/list":
            return self._json({{"jsonrpc": "2.0", "id": rid, "result": {{"tools": [
                {{"name": "ping", "description": "Ping",
                 "inputSchema": {{"type": "object", "properties": {{}}}}}}]}}}})
        if method == "tools/call":
            params = msg.get("params", {{}})
            if params.get("name") == "ping":
                return self._json({{"jsonrpc": "2.0", "id": rid, "result": {{
                    "content": [{{"type": "text", "text": "pong"}}]}}}})
            return self._json({{"jsonrpc": "2.0", "id": rid,
                               "error": {{"code": -32601, "message": "unknown tool"}}}})
        return self._json({{"jsonrpc": "2.0", "id": rid,
                           "error": {{"code": -32601, "message": "Method not found"}}}})

srv = HTTPServer(("127.0.0.1", 0), H)
PORT = srv.server_address[1]
print(PORT, flush=True)
srv.serve_forever()
"#
    )
}

/// Same discovery/`/authorize`/`/token` triad as [`server_script`], but `/mcp`
/// ALWAYS `401`s regardless of token — even a freshly consented one. Drives
/// the still-rejected-after-consent test: proves the one-shot interactive
/// retry in `connect_with_token` (`crates/sema/src/workflow_mcp.rs`) gates to
/// `needs-auth` on a second rejection instead of prompting again.
fn server_script_always_reject_mcp() -> String {
    r#"
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs, urlencode

PORT = None

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
            return self._json({"resource": self.base() + "/mcp",
                               "authorization_servers": [self.base()],
                               "scopes_supported": ["mcp:tools"]})
        if p.path == "/.well-known/oauth-authorization-server":
            return self._json({"issuer": self.base(),
                               "authorization_endpoint": self.base() + "/authorize",
                               "token_endpoint": self.base() + "/token",
                               "code_challenge_methods_supported": ["S256"]})
        if p.path == "/authorize":
            q = parse_qs(p.query)
            redirect_uri = q.get("redirect_uri", [""])[0]
            state = q.get("state", [""])[0]
            loc = redirect_uri + "?" + urlencode({"code": "authcode-always-reject-1",
                                                   "state": state})
            self.send_response(302)
            self.send_header("Location", loc)
            self.end_headers()
            return
        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        p = urlparse(self.path)
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b""
        if p.path == "/token":
            body = parse_qs(raw.decode())
            grant = body.get("grant_type", [""])[0]
            if grant == "authorization_code":
                return self._json({"access_token": "consented-but-still-rejected",
                                   "refresh_token": "consented-refresh-still-rejected",
                                   "token_type": "Bearer", "expires_in": 3600,
                                   "scope": "mcp:tools"})
            return self._json({"error": "unsupported_grant_type"}, code=400)
        if p.path != "/mcp":
            self.send_response(404)
            self.end_headers()
            return
        self.send_response(401)
        self.send_header(
            "WWW-Authenticate",
            'Bearer resource_metadata="%s/.well-known/oauth-protected-resource"' % self.base(),
        )
        self.end_headers()

srv = HTTPServer(("127.0.0.1", 0), H)
PORT = srv.server_address[1]
print(PORT, flush=True)
srv.serve_forever()
"#
    .to_string()
}

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

fn start_server_script(script: &str) -> (ServerGuard, u16) {
    let mut child = Command::new("python3")
        .args(["-c", script])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn python3 mock OAuth+MCP server");
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

fn start_server(deny: bool) -> (ServerGuard, u16) {
    start_server_script(&server_script(deny))
}

/// Seed a `:persist :run`-scoped credential for `url` into `<run_dir>/<run_id>/auth/`
/// — the same directory `store_for(McpPersist::Run, ...)` resolves to. Mirrors
/// `workflow_mcp_e2e_test.rs`'s helper of the same name — fixtures are cheaper
/// to duplicate across test files than to couple across them (this file's own
/// module doc comment gives the same reasoning for its mock server).
fn seed_run_scoped_credential(
    run_dir: &Path,
    run_id: &str,
    url: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_in: Option<u64>,
) {
    let dir = run_dir.join(run_id).join("auth");
    let store = ScopedFileStore::new(dir, auth_key_bytes());
    store
        .save(&StoredCredentials {
            server_url: url.to_string(),
            tokens: TokenSet::from_response(
                access_token.to_string(),
                refresh_token.map(str::to_string),
                expires_in,
                Some("mcp:tools".to_string()),
                sema_mcp::oauth::store::now_unix(),
            ),
            client_info: Some(ClientInfo {
                client_id: "test-client".to_string(),
                client_secret: None,
            }),
        })
        .expect("seed credential");
}

fn temp_run_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "sema-mcp-interactive-{}-{}-{tag}",
        std::process::id(),
        n
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

struct RunOutput {
    events: Vec<serde_json::Value>,
    result: serde_json::Value,
    /// Number of times the interactive browser opener ran during this
    /// `run_workflow` call — see [`counting_opener`]. Only meaningful when the
    /// test actually passed `counting_opener` as its `opener`.
    opener_hits: u64,
}

/// Counts invocations of the "browser" opener — layered on [`visiting_opener`]
/// so it still drives a real loopback redirect. Backed by a process-global
/// counter reset (and read back) inside `run_workflow`'s `SERIAL`-guarded
/// window, so cross-test values never leak: the one-shot-retry tests use this
/// to prove a still-rejected freshly-consented token never triggers a SECOND
/// interactive prompt.
static OPENER_HIT_COUNT: AtomicU64 = AtomicU64::new(0);

fn counting_opener(url: &str) -> Result<(), String> {
    OPENER_HIT_COUNT.fetch_add(1, Ordering::SeqCst);
    visiting_opener(url)
}

fn events_of<'a>(events: &'a [serde_json::Value], name: &str) -> Vec<&'a serde_json::Value> {
    events.iter().filter(|e| e["event"] == name).collect()
}

/// The "browser": a blocking GET that follows the authorization server's
/// redirect to our real loopback listener — the exact pattern
/// `crates/sema-mcp/tests/mcp_oauth_test.rs` and `mcp_oauth_connect_test.rs`
/// use to drive the OAuth callback without a real browser. A plain `fn` (not a
/// closure) because `set_interactive_login_opener` takes a `fn` pointer — see
/// its doc comment for why (the opener runs on a thread `LoopbackDriver`
/// spawns, and thread-locals don't propagate to a spawned thread).
fn visiting_opener(url: &str) -> Result<(), String> {
    reqwest::blocking::Client::new()
        .get(url)
        .send()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// A canary opener that panics if ever invoked — for the sandbox-denied test,
/// where the run-start interactive path must fall back to the headless gate
/// WITHOUT ever attempting to open a browser (real or fake).
fn panicking_opener(_url: &str) -> Result<(), String> {
    panic!("the browser opener must never run when the sandbox denies PROCESS");
}

/// Run `src` as a workflow against the REAL resolver with interactive auth
/// forced on, under the fixed-ts seam, into `run_dir/<run_id>/`. `sandbox`
/// mirrors `InterpreterBuilder::with_sandbox` (default `allow_all()` unless a
/// test needs to prove the `Caps::PROCESS` gate). Serialized via `SERIAL`
/// (env + the interactive-auth thread-locals are both process/thread-wide).
fn run_workflow(
    src: &str,
    run_dir: &Path,
    run_id: &str,
    sandbox: Sandbox,
    opener: fn(&str) -> Result<(), String>,
) -> RunOutput {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    OPENER_HIT_COUNT.store(0, Ordering::SeqCst);

    std::env::set_var("SEMA_WORKFLOW_FIXED_TS", "0");
    std::env::set_var("SEMA_WORKFLOW_RUN_ID", run_id);
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", run_dir);
    std::env::set_var("SEMA_WORKFLOW_CODE_VERSION", "");
    std::env::set_var("SEMA_WORKFLOW_ARGS_JSON", "{}");
    std::env::remove_var("SEMA_WORKFLOW_RESUME");
    std::env::set_var("SEMA_MCP_AUTH_KEY", auth_key_hex());

    sema::workflow_mcp::set_interactive_auth(true);
    sema::workflow_mcp::set_interactive_login_opener(Some(opener));

    let interp = InterpreterBuilder::new().with_sandbox(sandbox).build();
    sema::workflow_mcp::register_real_resolver();
    let _ = interp.eval_str(src);

    sema::workflow_mcp::set_interactive_login_opener(None);
    sema::workflow_mcp::set_interactive_auth(false);

    for v in [
        "SEMA_WORKFLOW_FIXED_TS",
        "SEMA_WORKFLOW_RUN_ID",
        "SEMA_WORKFLOW_RUN_DIR",
        "SEMA_WORKFLOW_CODE_VERSION",
        "SEMA_WORKFLOW_ARGS_JSON",
        "SEMA_MCP_AUTH_KEY",
    ] {
        std::env::remove_var(v);
    }

    let run = run_dir.join(run_id);
    let events = std::fs::read_to_string(run.join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid event json"))
        .collect();
    let result = std::fs::read_to_string(run.join("result.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let opener_hits = OPENER_HIT_COUNT.load(Ordering::SeqCst);
    RunOutput {
        events,
        result,
        opener_hits,
    }
}

// ── (1) happy path: no stored creds, TTY-forced interactive login completes ───

#[test]
fn interactive_login_completes_run_end_to_end_with_consented_source() {
    let (_server, port) = start_server(false);
    let url = format!("http://127.0.0.1:{port}/mcp");
    let run_dir = temp_run_dir("happy-path");
    let run_id = "interactive-happy";

    let src = format!(
        r#"
        (defworkflow triage
          "interactive e2e"
          {{:budget {{:usd 1.0}}
            :mcp {{srv {{:url "{url}"
                        :auth {{:scopes ["mcp:tools"] :client-id "test-client"}}
                        :persist :run}}}}}}
          (phase "Use")
          (checkpoint :out (mcp/call srv "ping" {{}}))
          {{:status :success :out (checkpoint :out)}})
        "#
    );

    let out = run_workflow(
        &src,
        &run_dir,
        run_id,
        Sandbox::allow_all(),
        visiting_opener,
    );

    // No headless gate at all — straight to a granted, consented connection.
    assert!(
        events_of(&out.events, "auth.required").is_empty(),
        "{:?}",
        out.events
    );
    let granted = events_of(&out.events, "auth.granted");
    assert_eq!(granted.len(), 1, "{:?}", out.events);
    assert_eq!(granted[0]["server"], "srv");
    assert_eq!(granted[0]["source"], "consented");

    assert_eq!(out.result["status"], "success");
    assert_eq!(out.result["out"], "pong");

    let ended = events_of(&out.events, "run.ended");
    assert_eq!(ended[0]["status"], "success");

    // The fresh session landed in the run-scoped store (`:persist :run`).
    let store = ScopedFileStore::new(run_dir.join(run_id).join("auth"), auth_key_bytes());
    let saved = store.load(&url).expect("interactive session persisted");
    assert_eq!(saved.tokens.access_token, "consented-token-xyz");

    let _ = std::fs::remove_dir_all(&run_dir);
}

// ── (2) opener/consent fails -> falls back to the headless needs-auth gate ────

#[test]
fn declined_consent_falls_back_to_needs_auth() {
    let (_server, port) = start_server(true); // DENY = True
    let url = format!("http://127.0.0.1:{port}/mcp");
    let run_dir = temp_run_dir("declined");
    let run_id = "interactive-declined";

    let src = format!(
        r#"
        (defworkflow triage
          "interactive e2e decline"
          {{:budget {{:usd 1.0}}
            :mcp {{srv {{:url "{url}"
                        :auth {{:scopes ["mcp:tools"] :client-id "test-client"}}
                        :persist :run}}}}}}
          (phase "Use")
          (checkpoint :ran #t)
          {{:status :success}})
        "#
    );

    let out = run_workflow(
        &src,
        &run_dir,
        run_id,
        Sandbox::allow_all(),
        visiting_opener,
    );

    assert!(events_of(&out.events, "auth.granted").is_empty());
    let required = events_of(&out.events, "auth.required");
    assert_eq!(required.len(), 1, "{:?}", out.events);
    assert_eq!(required[0]["server"], "srv");

    assert!(
        events_of(&out.events, "phase.started").is_empty(),
        "body must not run when the interactive login is declined"
    );
    assert_eq!(out.result["status"], "needs-auth");

    // Nothing landed in the scoped store — a declined consent persists nothing.
    let store = ScopedFileStore::new(run_dir.join(run_id).join("auth"), auth_key_bytes());
    assert!(store.load(&url).is_none());

    let _ = std::fs::remove_dir_all(&run_dir);
}

// ── (3) sandbox denies PROCESS -> never even tries the browser (self-review) ──

#[test]
fn sandbox_denying_process_falls_back_without_touching_the_opener() {
    let (_server, port) = start_server(false);
    let url = format!("http://127.0.0.1:{port}/mcp");
    let run_dir = temp_run_dir("sandbox-denied");
    let run_id = "interactive-sandbox-denied";

    let src = format!(
        r#"
        (defworkflow triage
          "interactive e2e sandbox-denied"
          {{:budget {{:usd 1.0}}
            :mcp {{srv {{:url "{url}"
                        :auth {{:scopes ["mcp:tools"] :client-id "test-client"}}
                        :persist :run}}}}}}
          (phase "Use")
          (checkpoint :ran #t)
          {{:status :success}})
        "#
    );

    // A `panicking_opener` proves the browser path is never entered: if the
    // sandbox gate were bypassed, the test would panic instead of merely
    // failing an assertion.
    let out = run_workflow(
        &src,
        &run_dir,
        run_id,
        Sandbox::deny(Caps::PROCESS),
        panicking_opener,
    );

    assert!(events_of(&out.events, "auth.granted").is_empty());
    let required = events_of(&out.events, "auth.required");
    assert_eq!(required.len(), 1, "{:?}", out.events);
    assert_eq!(out.result["status"], "needs-auth");

    let _ = std::fs::remove_dir_all(&run_dir);
}

// ── (4) one-shot retry: server-rejected but locally-fresh cached token ────────
// ── retries interactively ONCE and grants source:"consented" ──────────────────

#[test]
fn stale_cached_token_retries_interactively_and_grants_consented_source() {
    let (_server, port) = start_server(false); // RECOGNIZED = {"consented-token-xyz"}
    let url = format!("http://127.0.0.1:{port}/mcp");
    let run_dir = temp_run_dir("stale-cached-retry");
    let run_id = "interactive-stale-cached-retry";

    // Locally-fresh (unexpired) but server-unrecognized token: the "cached"
    // connect 401s and — interactive auth enabled — `connect_with_token`
    // (`crates/sema/src/workflow_mcp.rs`) retries ONCE through the
    // interactive login path, which succeeds and reconnects with the freshly
    // consented token.
    seed_run_scoped_credential(
        &run_dir,
        run_id,
        &url,
        "stale-locally-fresh-token",
        None,
        Some(3600),
    );

    let src = format!(
        r#"
        (defworkflow triage
          "interactive e2e stale-cached-retry"
          {{:budget {{:usd 1.0}}
            :mcp {{srv {{:url "{url}"
                        :auth {{:scopes ["mcp:tools"] :client-id "test-client"}}
                        :persist :run}}}}}}
          (phase "Use")
          (checkpoint :out (mcp/call srv "ping" {{}}))
          {{:status :success :out (checkpoint :out)}})
        "#
    );

    let out = run_workflow(
        &src,
        &run_dir,
        run_id,
        Sandbox::allow_all(),
        counting_opener,
    );

    assert!(
        events_of(&out.events, "auth.required").is_empty(),
        "{:?}",
        out.events
    );
    let granted = events_of(&out.events, "auth.granted");
    assert_eq!(granted.len(), 1, "{:?}", out.events);
    assert_eq!(granted[0]["server"], "srv");
    assert_eq!(granted[0]["source"], "consented");

    assert_eq!(out.result["status"], "success");
    assert_eq!(out.result["out"], "pong");

    assert_eq!(
        out.opener_hits, 1,
        "the interactive retry must open the browser exactly once"
    );

    let _ = std::fs::remove_dir_all(&run_dir);
}

// ── (5) one-shot retry: still-rejected consented token gates to needs-auth ────
// ── WITHOUT a second prompt (the one-shot guard, not recursion/looping) ───────

#[test]
fn still_rejected_after_consent_gates_to_needs_auth_without_second_prompt() {
    let (_server, port) = start_server_script(&server_script_always_reject_mcp());
    let url = format!("http://127.0.0.1:{port}/mcp");
    let run_dir = temp_run_dir("still-rejected");
    let run_id = "interactive-still-rejected";

    // Same setup as the retry-succeeds test above, but against a server that
    // 401s EVERY token, including the freshly consented one — so the retry's
    // own connect also fails. Proves `connect_with_token`'s one-shot guard:
    // that second `NeedsAuth` (source "consented") must gate straight to
    // `needs_auth` rather than retrying again.
    seed_run_scoped_credential(
        &run_dir,
        run_id,
        &url,
        "stale-locally-fresh-token",
        None,
        Some(3600),
    );

    let src = format!(
        r#"
        (defworkflow triage
          "interactive e2e still-rejected"
          {{:budget {{:usd 1.0}}
            :mcp {{srv {{:url "{url}"
                        :auth {{:scopes ["mcp:tools"] :client-id "test-client"}}
                        :persist :run}}}}}}
          (phase "Use")
          (checkpoint :ran #t)
          {{:status :success}})
        "#
    );

    let out = run_workflow(
        &src,
        &run_dir,
        run_id,
        Sandbox::allow_all(),
        counting_opener,
    );

    assert!(events_of(&out.events, "auth.granted").is_empty());
    let required = events_of(&out.events, "auth.required");
    assert_eq!(required.len(), 1, "{:?}", out.events);
    assert_eq!(required[0]["server"], "srv");

    assert!(
        events_of(&out.events, "phase.started").is_empty(),
        "body must not run when the retry is also rejected"
    );
    assert_eq!(out.result["status"], "needs-auth");

    assert_eq!(
        out.opener_hits, 1,
        "no second prompt after a still-rejected consent — the retry is one-shot"
    );

    let _ = std::fs::remove_dir_all(&run_dir);
}
