//! End-to-end tests for the CLI skin of the headless-precursor loop (plan §3, §5):
//! a run that needs auth exits with guidance, `sema mcp login --token` stores a
//! pre-issued token headlessly, and a re-run proceeds silently. Unlike
//! `workflow_mcp_e2e_test.rs` (in-process, `InterpreterBuilder`), every step here
//! is driven as a REAL subprocess of the built `sema` binary
//! (`env!("CARGO_BIN_EXE_sema")`), because the exit-code behavior under test
//! (`crates/sema/src/main.rs::run_workflow_command`) only exists at the process
//! boundary — it calls `std::process::exit`, so it cannot be observed in-process.
//!
//! The mock MCP-over-HTTP server is the same Python `http.server` script as
//! `workflow_mcp_e2e_test.rs` (a `401` + `WWW-Authenticate` challenge for an
//! unrecognized bearer, the JSON-RPC `initialize`/`tools/list`/`tools/call` triad
//! otherwise) — kept in sync deliberately rather than shared, since test fixtures
//! are cheaper to duplicate than to couple across files.
//!
//! Env-var discipline: every seam below (`HOME`, `SEMA_MCP_TOKEN_STORE`,
//! `SEMA_MCP_AUTH_KEY`, `SEMA_WORKFLOW_RUN_ID`) is set with `Command::env` on the
//! spawned `sema` subprocess ONLY — never `std::env::set_var` in this test
//! process — so parallel tests (in this binary or any other) can never race on
//! process-global env. `HOME` is overridden per-subprocess so the default MCP
//! token-store file (`$HOME/Library/Application Support/sema/mcp-auth.json` on
//! macOS) never touches the real user's config dir; `SEMA_MCP_TOKEN_STORE=file`
//! skips the OS keychain entirely (no macOS Keychain prompt, no CI hang).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// A minimal MCP-over-HTTP server: any `/mcp` POST without a recognized
/// `Authorization: Bearer <token>` gets a `401` + `WWW-Authenticate` challenge; a
/// recognized bearer gets the normal JSON-RPC `initialize`/`tools/list`/
/// `tools/call` triad, exposing one tool (`ping`) that echoes back `"pong"`.
/// `"valid-token-abc"` is the pre-issued token these tests hand to `sema mcp
/// login --token`.
const SERVER: &str = r#"
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

PORT = None
RECOGNIZED = {"valid-token-abc"}

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
        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        p = urlparse(self.path)
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b""
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
        msg = json.loads(raw) if raw else {}
        method = msg.get("method")
        rid = msg.get("id")
        if rid is None:
            self.send_response(202)
            self.end_headers()
            return
        if method == "initialize":
            return self._json(
                {"jsonrpc": "2.0", "id": rid, "result": {
                    "protocolVersion": "2025-11-25", "capabilities": {},
                    "serverInfo": {"name": "e2e-cli-server", "version": "1.0"}}},
                headers={"Mcp-Session-Id": "sess-cli-e2e"},
            )
        if method == "tools/list":
            return self._json({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
                {"name": "ping", "description": "Ping",
                 "inputSchema": {"type": "object", "properties": {}}}]}})
        if method == "tools/call":
            params = msg.get("params", {})
            if params.get("name") == "ping":
                return self._json({"jsonrpc": "2.0", "id": rid, "result": {
                    "content": [{"type": "text", "text": "pong"}]}})
            return self._json({"jsonrpc": "2.0", "id": rid,
                               "error": {"code": -32601, "message": "unknown tool"}})
        return self._json({"jsonrpc": "2.0", "id": rid,
                           "error": {"code": -32601, "message": "Method not found"}})

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

fn start_server() -> (ServerGuard, u16) {
    let mut child = Command::new("python3")
        .args(["-c", SERVER])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn python3 mock MCP server");
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

fn unique_temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "sema-cli-mcp-e2e-{}-{}-{tag}",
        std::process::id(),
        n
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// A fixed 32-byte key, as 64 hex chars, for `SEMA_MCP_AUTH_KEY` — a TEST key,
/// never used for anything real; each test's project dir is thrown away after.
fn auth_key_hex() -> String {
    "44".repeat(32)
}

/// Run the built `sema` binary as a subprocess: `cwd` (so `.sema/runs` and
/// `.sema/auth/<workflow>` land under the test's own temp dir, never the repo
/// tree), an isolated `home` (relocates the default MCP token-store file so it
/// never touches the real user's config dir), and `SEMA_MCP_TOKEN_STORE=file`
/// (skips the OS keychain). `extra_envs` supplies any additional per-invocation
/// seam (`SEMA_MCP_AUTH_KEY`, `SEMA_WORKFLOW_RUN_ID`) — set on this `Command`
/// only, never process-global, so parallel tests can't race.
fn run_sema(cwd: &Path, home: &Path, args: &[&str], extra_envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sema"));
    cmd.args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("SEMA_MCP_TOKEN_STORE", "file");
    for (k, v) in extra_envs {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run sema subprocess")
}

fn events_of(events: &[serde_json::Value], name: &str) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|e| e["event"] == name)
        .cloned()
        .collect()
}

fn read_events(run_dir: &Path, run_id: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(run_dir.join(run_id).join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid event json"))
        .collect()
}

fn read_result(run_dir: &Path, run_id: &str) -> serde_json::Value {
    std::fs::read_to_string(run_dir.join(run_id).join("result.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

// ── (A) the loop: needs-auth exit 2 -> mcp login --token -> re-run exit 0 ──────

#[test]
fn cli_headless_login_loop_needs_auth_then_login_token_then_clean_rerun() {
    let (_server, port) = start_server();
    let url = format!("http://127.0.0.1:{port}/mcp");

    let project = unique_temp_dir("loop-project");
    let home = unique_temp_dir("loop-home");
    let key = auth_key_hex();

    let workflow_src = format!(
        r#"
        (defworkflow triage-cli
          "cli e2e loop"
          {{:budget {{:usd 1.0}}
            :mcp {{gated {{:url "{url}" :auth {{:scopes ["mcp:tools"]}} :persist :workflow}}}}}}
          (phase "Use")
          (checkpoint :out (mcp/call gated "ping" {{}}))
          {{:status :success :out (checkpoint :out)}})
        "#
    );
    std::fs::write(project.join("triage.sema"), &workflow_src).unwrap();

    // ── run 1: no credentials anywhere -> exit 2, stderr names the login command ──
    let out1 = run_sema(
        &project,
        &home,
        &["workflow", "run", "triage.sema"],
        &[
            ("SEMA_MCP_AUTH_KEY", key.as_str()),
            ("SEMA_WORKFLOW_RUN_ID", "cli-run-1"),
        ],
    );
    let stderr1 = String::from_utf8_lossy(&out1.stderr);
    assert_eq!(out1.status.code(), Some(2), "run 1 stderr: {stderr1}");
    assert!(
        stderr1.contains(&format!("sema mcp login {url}")),
        "stderr must name the exact login command: {stderr1}"
    );
    assert!(
        stderr1.contains("gated"),
        "stderr must name the server alias: {stderr1}"
    );

    // ── sema mcp login --token: pre-issued token, no browser/device flow ──────────
    let out_login = run_sema(
        &project,
        &home,
        &["mcp", "login", &url, "--token", "valid-token-abc"],
        &[],
    );
    let login_stdout = String::from_utf8_lossy(&out_login.stdout);
    let login_stderr = String::from_utf8_lossy(&out_login.stderr);
    assert!(
        out_login.status.success(),
        "login failed: stdout={login_stdout} stderr={login_stderr}"
    );
    assert!(
        !login_stdout.contains("valid-token-abc") && !login_stderr.contains("valid-token-abc"),
        "the token must never be echoed: stdout={login_stdout} stderr={login_stderr}"
    );

    // ── run 2: same store -> exit 0, cached grant, scoped store now populated ─────
    let out2 = run_sema(
        &project,
        &home,
        &["workflow", "run", "triage.sema"],
        &[
            ("SEMA_MCP_AUTH_KEY", key.as_str()),
            ("SEMA_WORKFLOW_RUN_ID", "cli-run-2"),
        ],
    );
    assert_eq!(
        out2.status.code(),
        Some(0),
        "run 2 stderr: {}",
        String::from_utf8_lossy(&out2.stderr)
    );

    let run_dir = project.join(".sema/runs");
    let events = read_events(&run_dir, "cli-run-2");
    let granted = events_of(&events, "auth.granted");
    assert_eq!(granted.len(), 1, "{events:?}");
    assert_eq!(granted[0]["server"], "gated");
    assert_eq!(granted[0]["source"], "cached");

    let result = read_result(&run_dir, "cli-run-2");
    assert_eq!(result["status"], "success");
    assert_eq!(result["out"], "pong");

    // The scoped `:persist :workflow` store now holds the credential imported
    // from the default store `sema mcp login` wrote to.
    let scoped_dir = project.join(".sema/auth/triage-cli");
    let entries: Vec<_> = std::fs::read_dir(&scoped_dir)
        .unwrap_or_else(|e| panic!("scoped store dir missing at {}: {e}", scoped_dir.display()))
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one imported ciphertext file in {}",
        scoped_dir.display()
    );
    let ciphertext_path = entries[0].path();
    assert_eq!(
        ciphertext_path.extension().and_then(|e| e.to_str()),
        Some("json")
    );
    let ciphertext = std::fs::read_to_string(&ciphertext_path).unwrap();
    assert!(
        !ciphertext.contains("valid-token-abc"),
        "the scoped store file must hold ciphertext, never the plaintext token: {ciphertext}"
    );

    let _ = std::fs::remove_dir_all(&project);
    let _ = std::fs::remove_dir_all(&home);
}

// ── (B) redaction guard: metadata.json never carries the secret header value ──

#[test]
fn cli_redacts_mcp_header_secret_in_metadata_json() {
    let (_server, port) = start_server();
    let url = format!("http://127.0.0.1:{port}/mcp");

    let project = unique_temp_dir("redact-project");
    let home = unique_temp_dir("redact-home");
    let key = auth_key_hex();

    // `gated` mirrors scenario A's auth server; `leaky` is a SECOND, no-`:auth`
    // server on the same workflow whose meta carries a secret-looking header.
    // The run's overall outcome doesn't matter here — `metadata.json` is written
    // unconditionally at run start, before any :mcp resolution — only the guard
    // that the header VALUE never reaches disk is under test.
    let workflow_src = format!(
        r#"
        (defworkflow redact-cli
          "cli e2e redaction guard"
          {{:budget {{:usd 1.0}}
            :mcp {{gated {{:url "{url}" :auth {{:scopes ["mcp:tools"]}} :persist :workflow}}
                   leaky {{:url "{url}" :headers {{"X-Extra" "sekrit-value"}}}}}}}}
          (phase "Use")
          (checkpoint :ran #t)
          {{:status :success}})
        "#
    );
    std::fs::write(project.join("redact.sema"), &workflow_src).unwrap();

    let _ = run_sema(
        &project,
        &home,
        &["workflow", "run", "redact.sema"],
        &[
            ("SEMA_MCP_AUTH_KEY", key.as_str()),
            ("SEMA_WORKFLOW_RUN_ID", "redact-run"),
        ],
    );

    let metadata_path = project.join(".sema/runs/redact-run/metadata.json");
    let metadata_text = std::fs::read_to_string(&metadata_path)
        .unwrap_or_else(|e| panic!("metadata.json missing at {}: {e}", metadata_path.display()));

    assert!(
        !metadata_text.contains("sekrit-value"),
        "the secret header value must never reach metadata.json (redact_meta_secrets / \
         value_to_json_lossy keyword-key contract): {metadata_text}"
    );

    let metadata: serde_json::Value = serde_json::from_str(&metadata_text).unwrap();
    assert_eq!(
        metadata["meta"]["mcp"]["leaky"]["headers"]["X-Extra"],
        "<redacted>"
    );
    // The header KEY (which header was configured) survives — only the value.
    assert!(metadata["meta"]["mcp"]["leaky"]["headers"]
        .as_object()
        .expect("headers must still be an object")
        .contains_key("X-Extra"));

    let _ = std::fs::remove_dir_all(&project);
    let _ = std::fs::remove_dir_all(&home);
}

// ── (C) exit-code stability: no :mcp declared -> 0/1 exactly as before ─────────

#[test]
fn cli_plain_workflow_without_mcp_exit_codes_unchanged() {
    let project = unique_temp_dir("plain-project");
    let home = unique_temp_dir("plain-home");

    let success_src = r#"
        (defworkflow plain-cli
          "no mcp here"
          {:budget {:usd 1.0}}
          (phase "Work")
          (checkpoint :x 1)
          {:status :success :x (checkpoint :x)})
    "#;
    std::fs::write(project.join("plain.sema"), success_src).unwrap();

    let out_ok = run_sema(
        &project,
        &home,
        &["workflow", "run", "plain.sema"],
        &[("SEMA_WORKFLOW_RUN_ID", "plain-run-ok")],
    );
    assert_eq!(
        out_ok.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out_ok.stderr)
    );
    let result_ok = read_result(&project.join(".sema/runs"), "plain-run-ok");
    assert_eq!(result_ok["status"], "success");
    assert_eq!(result_ok["x"], 1);

    let failing_src = r#"
        (defworkflow plain-cli-fail
          "no mcp here, fails"
          {:budget {:usd 1.0}}
          (phase "Work")
          {:status :failed :error "boom"})
    "#;
    std::fs::write(project.join("plain-fail.sema"), failing_src).unwrap();

    let out_fail = run_sema(
        &project,
        &home,
        &["workflow", "run", "plain-fail.sema"],
        &[("SEMA_WORKFLOW_RUN_ID", "plain-run-fail")],
    );
    assert_eq!(
        out_fail.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out_fail.stderr)
    );

    let _ = std::fs::remove_dir_all(&project);
    let _ = std::fs::remove_dir_all(&home);
}
