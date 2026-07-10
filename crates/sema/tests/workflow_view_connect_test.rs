//! End-to-end tests for the dashboard's one-click Connect/Forget write
//! endpoints (`sema::workflow_view`'s `POST /api/run/:id/auth/:alias/connect`
//! and `.../forget`, plus their session-token hardening) — Task 10 of
//! `docs/plans/2026-06-24-workflow-mcp-auth.md` §5/§8.
//!
//! Drives the REAL viewer server in-process via `sema::workflow_view::serve_test`
//! (an ephemeral-port bind + injectable browser opener), against a local mock
//! OAuth authorization server. The mock server + `visiting_opener` pattern
//! mirror `workflow_mcp_interactive_test.rs`'s harness (itself modeled on
//! `crates/sema-mcp/tests/mcp_oauth_test.rs`): a Python `http.server` script
//! printing its bound port on stdout, killed via a `Drop` guard, with an
//! `/authorize` GET that redirects to the loopback callback with either a code
//! or `error=access_denied`.
//!
//! Env-var discipline: `SEMA_WORKFLOW_RUN_DIR`/`SEMA_MCP_AUTH_KEY` are
//! process-global, so every test funnels through [`run_dir_env_guard`], which
//! holds a process-wide mutex for its whole set/run/clear window — no parallel
//! test IN THIS BINARY can interleave.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sema_mcp::oauth::scoped::ScopedFileStore;
use sema_mcp::oauth::store::TokenStore;

static SERIAL: Mutex<()> = Mutex::new(());

/// A fixed 32-byte key, as 64 hex chars — a TEST key, never used for anything
/// real; each test's run dir is thrown away afterward.
fn auth_key_hex() -> String {
    "44".repeat(32)
}

fn auth_key_bytes() -> [u8; 32] {
    let hex = auth_key_hex();
    let mut k = [0u8; 32];
    for (i, byte) in k.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    k
}

/// Hold `SERIAL` and set the two process-global env vars `crate::workflow_mcp`'s
/// `store_for` reads for `:persist :run` — mirrors
/// `workflow_mcp_e2e_test.rs`/`workflow_mcp_interactive_test.rs`'s own
/// `run_workflow` env-var window, scoped here to just what `connect`/`forget`
/// need (no workflow run is actually executed by these tests).
// The guard is held purely for its lock/Drop lifetime — never read.
struct EnvGuard<'a>(#[allow(dead_code)] std::sync::MutexGuard<'a, ()>);

fn run_dir_env_guard(run_dir: &Path) -> EnvGuard<'static> {
    let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", run_dir);
    std::env::set_var("SEMA_MCP_AUTH_KEY", auth_key_hex());
    EnvGuard(g)
}

impl Drop for EnvGuard<'_> {
    fn drop(&mut self) {
        std::env::remove_var("SEMA_WORKFLOW_RUN_DIR");
        std::env::remove_var("SEMA_MCP_AUTH_KEY");
    }
}

/// The mock authorization server: RFC 9728/8414 discovery, `/authorize`
/// (redirects to the loopback callback with a code, or `error=access_denied`
/// when `deny`), and `/token` accepting the `authorization_code` grant. No
/// `/mcp` endpoint at all — `connect` only ever runs the OAuth exchange
/// (`sema_mcp::login_interactive`), never an actual `mcp/connect`, so there is
/// nothing here for it to reach. `authorize_delay_secs` optionally pads the
/// `/authorize` response so a test has a reliable window to observe the flow
/// still `"connecting"`; `max_authorize_hits` (0 = unlimited) turns a SECOND
/// `/authorize` hit into a denial — the double-connect test's proof that only
/// one real flow ever ran.
fn server_script(deny: bool, authorize_delay_secs: f64, max_authorize_hits: u32) -> String {
    let deny_py = if deny { "True" } else { "False" };
    format!(
        r#"
import json
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs, urlencode

PORT = None
DENY = {deny_py}
AUTHORIZE_DELAY = {authorize_delay_secs}
MAX_AUTHORIZE_HITS = {max_authorize_hits}
AUTHORIZE_HITS = 0
RECOGNIZED = {{"connect-token-xyz"}}

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
        global AUTHORIZE_HITS
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
            AUTHORIZE_HITS += 1
            too_many = MAX_AUTHORIZE_HITS > 0 and AUTHORIZE_HITS > MAX_AUTHORIZE_HITS
            q = parse_qs(p.query)
            redirect_uri = q.get("redirect_uri", [""])[0]
            state = q.get("state", [""])[0]
            if DENY or too_many:
                loc = redirect_uri + "?" + urlencode({{"error": "access_denied",
                                                       "error_description": "too many authorize hits" if too_many else "user declined",
                                                       "state": state}})
            else:
                if AUTHORIZE_DELAY > 0:
                    time.sleep(AUTHORIZE_DELAY)
                loc = redirect_uri + "?" + urlencode({{"code": "authcode-connect-1",
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
            if body.get("grant_type", [""])[0] == "authorization_code":
                return self._json({{"access_token": "connect-token-xyz",
                                   "refresh_token": "connect-refresh-xyz",
                                   "token_type": "Bearer", "expires_in": 3600,
                                   "scope": "mcp:tools"}})
            return self._json({{"error": "unsupported_grant_type"}}, code=400)
        self.send_response(404)
        self.end_headers()

srv = HTTPServer(("127.0.0.1", 0), H)
PORT = srv.server_address[1]
print(PORT, flush=True)
srv.serve_forever()
"#
    )
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

fn start_oauth_server(
    deny: bool,
    authorize_delay_secs: f64,
    max_authorize_hits: u32,
) -> (ServerGuard, u16) {
    let mut child = Command::new("python3")
        .args([
            "-c",
            &server_script(deny, authorize_delay_secs, max_authorize_hits),
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn python3 mock OAuth server");
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

fn temp_run_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "sema-wf-view-connect-{}-{}-{tag}",
        std::process::id(),
        n
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The "browser": a blocking GET that follows the authorization server's
/// redirect to the real loopback listener `login_interactive` binds — the
/// exact pattern `workflow_mcp_interactive_test.rs`'s `visiting_opener` (and
/// `crates/sema-mcp/tests/mcp_oauth_test.rs`) use to drive the OAuth callback
/// without a real browser. A plain `fn` (not a closure), matching
/// `sema::workflow_view::connect::TestOpenerFn`'s shape.
fn visiting_opener(url: &str) -> Result<(), String> {
    reqwest::blocking::Client::new()
        .get(url)
        .send()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// A canary opener that panics if invoked — proves a route returned before
/// ever reaching the login flow (403/404 paths).
fn panicking_opener(_url: &str) -> Result<(), String> {
    panic!("the browser opener must never run on a rejected connect request");
}

/// Write `<run_dir>/<run_id>/metadata.json` + `events.jsonl` declaring one
/// HTTP `:mcp` alias with `:auth`, `:persist :run` (self-contained under this
/// test's own temp dir — no `.sema/auth/<workflow>/` involvement), gated
/// (`auth.required`, never granted) in the journal — the same "needs-consent"
/// baseline `workflow_view/auth.rs`'s tests use.
fn write_gated_run_fixture(run_dir: &Path, run_id: &str, workflow: &str, alias: &str, url: &str) {
    let dir = run_dir.join(run_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("metadata.json"),
        format!(
            r#"{{"workflow":"{workflow}","meta":{{"mcp":{{"{alias}":{{
                "url":"{url}",
                "auth":{{"scopes":["mcp:tools"],"client-id":"test-client"}},
                "persist":"run"
            }}}}}}}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("events.jsonl"),
        format!(
            r#"{{"event":"auth.required","seq":0,"ts":"0","server":"{alias}","scopes":["mcp:tools"],"persist":"run"}}"#
        ),
    )
    .unwrap();
}

/// Write a fixture declaring a STDIO alias (no `:url`) — used by the
/// not-an-http-server (400) test.
fn write_stdio_run_fixture(run_dir: &Path, run_id: &str, alias: &str) {
    let dir = run_dir.join(run_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("metadata.json"),
        format!(r#"{{"workflow":"w","meta":{{"mcp":{{"{alias}":{{"command":"npx"}}}}}}}}"#),
    )
    .unwrap();
}

async fn get_auth_json(client: &reqwest::Client, base: &str, run_id: &str) -> serde_json::Value {
    client
        .get(format!("{base}/api/run/{run_id}/auth"))
        .send()
        .await
        .expect("GET /auth")
        .json()
        .await
        .expect("valid json")
}

fn row_for<'a>(rows: &'a serde_json::Value, alias: &str) -> &'a serde_json::Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["alias"] == alias)
        .unwrap_or_else(|| panic!("no row for alias {alias:?} in {rows}"))
}

/// Poll `GET …/auth` until `alias`'s status leaves `"connecting"` (or the
/// deadline passes), returning the last-seen rows. The dashboard panel itself
/// polls the same endpoint every second (see `index.html`'s `init()`); tests
/// poll faster so they don't sit through that whole interval.
async fn wait_for_terminal_status(
    client: &reqwest::Client,
    base: &str,
    run_id: &str,
    alias: &str,
    timeout: Duration,
) -> serde_json::Value {
    let deadline = Instant::now() + timeout;
    loop {
        let rows = get_auth_json(client, base, run_id).await;
        let status = row_for(&rows, alias)["status"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if status != "connecting" {
            return rows;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for {alias} to leave \"connecting\": {rows}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ── (1) happy path: connect → flow → scoped store populated → GET authorized ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_happy_path_populates_scoped_store_and_get_shows_authorized() {
    let (_oauth, port) = start_oauth_server(false, 0.0, 0);
    let url = format!("http://127.0.0.1:{port}/mcp");
    let run_dir = temp_run_dir("happy");
    let run_id = "run-happy";
    let alias = "asana";
    write_gated_run_fixture(&run_dir, run_id, "w", alias, &url);

    let _env = run_dir_env_guard(&run_dir);
    let (addr, token) = sema::workflow_view::serve_test(run_dir.clone(), Some(visiting_opener))
        .await
        .expect("bind viewer server");
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    // Baseline: needs-consent (no override yet).
    let before = get_auth_json(&client, &base, run_id).await;
    assert_eq!(row_for(&before, alias)["status"], "needs-consent");

    let resp = client
        .post(format!("{base}/api/run/{run_id}/auth/{alias}/connect"))
        .header("X-Sema-View-Token", &token)
        .send()
        .await
        .expect("POST connect");
    assert_eq!(resp.status(), 202);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "connecting");

    let rows =
        wait_for_terminal_status(&client, &base, run_id, alias, Duration::from_secs(10)).await;
    let row = row_for(&rows, alias);
    assert_eq!(row["status"], "authorized", "{rows}");
    assert!(row["expires_at"].is_number(), "{rows}");
    // No token material anywhere in the response.
    let text = rows.to_string();
    assert!(!text.contains("connect-token-xyz"), "{text}");
    assert!(!text.contains("connect-refresh-xyz"), "{text}");

    // The scoped `:persist :run` store now holds the fresh credential.
    let store = ScopedFileStore::new(run_dir.join(run_id).join("auth"), auth_key_bytes());
    let saved = store.load(&url).expect("connect persisted a credential");
    assert_eq!(saved.tokens.access_token, "connect-token-xyz");

    let _ = std::fs::remove_dir_all(&run_dir);
}

// ── (2) missing/wrong token → 403, NO flow started ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_or_wrong_token_is_403_and_starts_no_flow() {
    let (_oauth, port) = start_oauth_server(false, 0.0, 0);
    let url = format!("http://127.0.0.1:{port}/mcp");
    let run_dir = temp_run_dir("bad-token");
    let run_id = "run-bad-token";
    let alias = "asana";
    write_gated_run_fixture(&run_dir, run_id, "w", alias, &url);

    let _env = run_dir_env_guard(&run_dir);
    // A panicking opener: if a flow were (wrongly) started, the background
    // task would call it and this test would panic instead of merely failing
    // an assertion. `token` (the real, correct one) is deliberately unused —
    // this test only ever sends no header or a wrong one.
    let (addr, _token) = sema::workflow_view::serve_test(run_dir.clone(), Some(panicking_opener))
        .await
        .expect("bind viewer server");
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    // No header at all.
    let resp = client
        .post(format!("{base}/api/run/{run_id}/auth/{alias}/connect"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // Wrong value.
    let resp = client
        .post(format!("{base}/api/run/{run_id}/auth/{alias}/connect"))
        .header("X-Sema-View-Token", "not-the-real-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // Give any (wrongly) spawned background task a moment, then confirm the
    // status is still exactly the untouched journal baseline — no flow ever
    // landed in the in-memory map.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let rows = get_auth_json(&client, &base, run_id).await;
    assert_eq!(row_for(&rows, alias)["status"], "needs-consent", "{rows}");

    // Same 403 discipline on forget.
    let resp = client
        .post(format!("{base}/api/run/{run_id}/auth/{alias}/forget"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    let _ = std::fs::remove_dir_all(&run_dir);
}

// ── (3) undeclared alias → 404, no flow; declared-but-stdio → 400 ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undeclared_alias_is_404_and_stdio_alias_is_400() {
    let run_dir = temp_run_dir("undeclared");
    let run_id = "run-undeclared";
    write_stdio_run_fixture(&run_dir, run_id, "fsserver");

    let _env = run_dir_env_guard(&run_dir);
    let (addr, token) = sema::workflow_view::serve_test(run_dir.clone(), Some(panicking_opener))
        .await
        .expect("bind viewer server");
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    // Not declared in this run's manifest at all.
    let resp = client
        .post(format!("{base}/api/run/{run_id}/auth/nope/connect"))
        .header("X-Sema-View-Token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // Declared, but a stdio spec — never an HTTP OAuth flow.
    let resp = client
        .post(format!("{base}/api/run/{run_id}/auth/fsserver/connect"))
        .header("X-Sema-View-Token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let _ = std::fs::remove_dir_all(&run_dir);
}

// ── (4) double-connect while pending → a single flow, one /authorize hit ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn double_connect_while_pending_starts_a_single_flow() {
    // A generous delay on /authorize gives the second POST a wide window to
    // land while the first flow is still "connecting", and MAX_AUTHORIZE_HITS
    // = 1 means a genuine second flow would poison the run with a denial —
    // so ending up "authorized" is proof only one flow ever ran.
    let (_oauth, port) = start_oauth_server(false, 0.5, 1);
    let url = format!("http://127.0.0.1:{port}/mcp");
    let run_dir = temp_run_dir("double-connect");
    let run_id = "run-double-connect";
    let alias = "asana";
    write_gated_run_fixture(&run_dir, run_id, "w", alias, &url);

    let _env = run_dir_env_guard(&run_dir);
    let (addr, token) = sema::workflow_view::serve_test(run_dir.clone(), Some(visiting_opener))
        .await
        .expect("bind viewer server");
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    let connect = |client: reqwest::Client,
                   base: String,
                   run_id: &'static str,
                   alias: &'static str,
                   token: String| async move {
        client
            .post(format!("{base}/api/run/{run_id}/auth/{alias}/connect"))
            .header("X-Sema-View-Token", token)
            .send()
            .await
            .unwrap()
    };

    let r1 = connect(client.clone(), base.clone(), run_id, alias, token.clone()).await;
    assert_eq!(r1.status(), 202);
    let body1: serde_json::Value = r1.json().await.unwrap();
    assert_eq!(body1["status"], "connecting");

    // Fired immediately after the first 202 — the flow-map insert happened
    // synchronously before that response was sent, so this is guaranteed to
    // observe "Connecting" already recorded (see connect.rs's handle_connect).
    let r2 = connect(client.clone(), base.clone(), run_id, alias, token.clone()).await;
    assert_eq!(r2.status(), 202);
    let body2: serde_json::Value = r2.json().await.unwrap();
    assert_eq!(body2["status"], "connecting");

    let rows =
        wait_for_terminal_status(&client, &base, run_id, alias, Duration::from_secs(10)).await;
    assert_eq!(
        row_for(&rows, alias)["status"],
        "authorized",
        "a second concurrent flow would have poisoned this via MAX_AUTHORIZE_HITS: {rows}"
    );

    let _ = std::fs::remove_dir_all(&run_dir);
}

// ── (5) forget → stores emptied + GET back to needs-consent ───────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forget_empties_stores_and_get_reverts_to_needs_consent() {
    let (_oauth, port) = start_oauth_server(false, 0.0, 0);
    let url = format!("http://127.0.0.1:{port}/mcp");
    let run_dir = temp_run_dir("forget");
    let run_id = "run-forget";
    let alias = "asana";
    write_gated_run_fixture(&run_dir, run_id, "w", alias, &url);

    let _env = run_dir_env_guard(&run_dir);
    let (addr, token) = sema::workflow_view::serve_test(run_dir.clone(), Some(visiting_opener))
        .await
        .expect("bind viewer server");
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    // Connect first so there is something to forget.
    client
        .post(format!("{base}/api/run/{run_id}/auth/{alias}/connect"))
        .header("X-Sema-View-Token", &token)
        .send()
        .await
        .unwrap();
    let rows =
        wait_for_terminal_status(&client, &base, run_id, alias, Duration::from_secs(10)).await;
    assert_eq!(row_for(&rows, alias)["status"], "authorized", "{rows}");

    let store = ScopedFileStore::new(run_dir.join(run_id).join("auth"), auth_key_bytes());
    assert!(
        store.load(&url).is_some(),
        "precondition: credential persisted"
    );

    let resp = client
        .post(format!("{base}/api/run/{run_id}/auth/{alias}/forget"))
        .header("X-Sema-View-Token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "forgotten");

    // The scoped store no longer has it…
    assert!(
        store.load(&url).is_none(),
        "forget must delete the scoped credential"
    );

    // …and the panel falls back to the journal's needs-consent (this
    // fixture's journal never recorded an auth.granted — see
    // write_gated_run_fixture), not a stale "authorized".
    let rows = get_auth_json(&client, &base, run_id).await;
    assert_eq!(row_for(&rows, alias)["status"], "needs-consent", "{rows}");

    // Best-effort: forgetting again (nothing left to delete) is still success.
    let resp = client
        .post(format!("{base}/api/run/{run_id}/auth/{alias}/forget"))
        .header("X-Sema-View-Token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let _ = std::fs::remove_dir_all(&run_dir);
}

// ── (6) declined consent (the opener runs, the AS denies) → GET shows failed
//        with reason. `login_interactive`'s loopback wait only resolves once
//        the callback is hit, so a genuinely never-returning opener would
//        hang out its full (hardcoded, 300s) timeout — a denial redirect is
//        the fast, deterministic way to exercise this failure path, and is
//        exactly the mechanism `workflow_mcp_interactive_test.rs`'s own
//        `declined_consent_falls_back_to_needs_auth` test uses. ─────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declined_consent_shows_failed_with_reason_never_a_secret() {
    let (_oauth, port) = start_oauth_server(true, 0.0, 0); // DENY = true
    let url = format!("http://127.0.0.1:{port}/mcp");
    let run_dir = temp_run_dir("declined");
    let run_id = "run-declined";
    let alias = "asana";
    write_gated_run_fixture(&run_dir, run_id, "w", alias, &url);

    let _env = run_dir_env_guard(&run_dir);
    let (addr, token) = sema::workflow_view::serve_test(run_dir.clone(), Some(visiting_opener))
        .await
        .expect("bind viewer server");
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    client
        .post(format!("{base}/api/run/{run_id}/auth/{alias}/connect"))
        .header("X-Sema-View-Token", &token)
        .send()
        .await
        .unwrap();

    let rows =
        wait_for_terminal_status(&client, &base, run_id, alias, Duration::from_secs(10)).await;
    let row = row_for(&rows, alias);
    assert_eq!(row["status"], "failed", "{rows}");
    let reason = row["reason"].as_str().expect("failed row carries a reason");
    assert!(!reason.is_empty());
    assert!(!reason.contains("connect-token-xyz"), "{reason}");
    assert!(!reason.contains("connect-refresh-xyz"), "{reason}");

    // Nothing landed in the scoped store — a declined consent persists nothing.
    let store = ScopedFileStore::new(run_dir.join(run_id).join("auth"), auth_key_bytes());
    assert!(store.load(&url).is_none());

    let _ = std::fs::remove_dir_all(&run_dir);
}
