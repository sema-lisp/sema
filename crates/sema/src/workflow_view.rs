//! `sema workflow view` — a tiny read-only web viewer over workflow run journals.
//!
//! Spike (scope doc `docs/plans/archive/2026-06-23-workflow-dashboard-scope.md`, Option A):
//! a self-contained AlpineJS tree viewer that `fetch()`es a run's `events.jsonl` and
//! renders the Claude-Code-`/workflows`-style live tree, served by a minimal
//! loopback HTTP server. The richer Option B (SQLite projection + server-side
//! live-tail cursor) is a later upgrade; this spike parses the frozen journal
//! client-side and polls while a run is still `running`.
//!
//! Security: loopback-only by default. GET routes are unauthenticated — the same
//! trusted-local-developer tool model the notebook server documents. Binding a
//! non-loopback host exposes the run directory's contents (and, per below, the
//! write endpoints) to the network; that is the operator's responsibility.
//!
//! **Write-route hardening (plan §8):** `connect`/`forget` (`connect` module)
//! can trigger a real OAuth consent screen, so "loopback + no auth" alone is not
//! enough for those two routes — a malicious local page could otherwise POST to
//! them blind. At startup this server mints a random 32-hex session token
//! (`sema_mcp::random_hex_token`) and substitutes it into the served HTML in
//! place of the `__SEMA_VIEW_TOKEN__` placeholder (see `route`'s `"/"` case).
//! Every write route requires header `X-Sema-View-Token: <token>` matching
//! exactly; missing or wrong is a `403` with no side effects. This is
//! deliberately cheap, not a rewrite: the custom header ALSO forces a CORS
//! preflight on any cross-origin request, which this server never answers (no
//! `Access-Control-Allow-Origin` handling at all), so a third-party page's
//! browser-issued `fetch` can't even reach the route; the token defeats a
//! same-origin/drive-by guess. GET routes stay unauthenticated (read-only,
//! same trust model as before).
//!
//! No new crate dependency: a ~hand-rolled HTTP/1.1 handler over the `tokio` net/io
//! the binary already pulls in (the notebook uses axum; a handful of routes does
//! not need it).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub mod auth;
mod connect;
pub mod ingest;

use connect::ServerState;

/// The common write/read-route response shape: `(status line, content-type,
/// body)`. `status`/`content_type` are always `'static` literals; `body` is
/// the one owned/dynamic piece.
pub(crate) type JsonResponse = (&'static str, &'static str, Vec<u8>);

const INDEX_HTML: &str = include_str!("workflow_view/index.html");
const ALPINE_JS: &str = include_str!("workflow_view/alpine.min.js");
/// Substituted for the real per-process session token at response time (see
/// the module doc's §8 note). Must match the placeholder literal embedded in
/// `workflow_view/index.html`'s `<script>`.
const VIEW_TOKEN_PLACEHOLDER: &str = "__SEMA_VIEW_TOKEN__";

/// Serve the viewer for the runs under `run_dir`, exiting the process on a bind
/// failure. Used by the standalone `sema workflow view` command.
pub async fn serve(run_dir: PathBuf, host: &str, port: u16) {
    if let Err(e) = serve_result(run_dir, host, port, true).await {
        eprintln!("sema workflow view: {e}");
        std::process::exit(1);
    }
}

/// Serve the viewer, returning a bind error instead of exiting — so an embedded
/// viewer (`workflow run --view`) can degrade to a warning without killing the run.
/// `announce` prints the URL on a successful bind.
pub async fn serve_result(
    run_dir: PathBuf,
    host: &str,
    port: u16,
    announce: bool,
) -> std::io::Result<()> {
    let addr = format!("{host}:{port}");
    let listener = TcpListener::bind(&addr).await?;
    if announce {
        println!("Sema workflow viewer:  http://{addr}");
        println!("  runs: {}", run_dir.display());
    }
    let state = Arc::new(ServerState::new(
        run_dir,
        sema_mcp::random_hex_token(),
        None,
    ));
    accept_loop(listener, state).await;
    Ok(())
}

/// TEST-ONLY seam: bind an ephemeral loopback port and serve it in the
/// background, returning the bound address and the minted session token so an
/// integration test's own HTTP client can drive the write endpoints (with the
/// correct `X-Sema-View-Token`) without guessing or parsing it out of the HTML.
/// `opener` overrides the browser opener `connect` hands to
/// `sema_mcp::login_interactive` — the same `LoopbackDriver::with_opener` seam
/// `workflow_mcp_interactive_test.rs`'s `visiting_opener` drives, extended to
/// this server. `None` uses the real, sandbox-gated opener (never exercised in
/// tests — CI has no browser and no display).
///
/// Not used by any CLI path; `serve`/`serve_result` above are what `sema
/// workflow view` actually runs.
pub async fn serve_test(
    run_dir: PathBuf,
    opener: Option<connect::TestOpenerFn>,
) -> std::io::Result<(std::net::SocketAddr, String)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let token = sema_mcp::random_hex_token();
    let state = Arc::new(ServerState::new(run_dir, token.clone(), opener));
    tokio::spawn(accept_loop(listener, state));
    Ok((addr, token))
}

async fn accept_loop(listener: TcpListener, state: Arc<ServerState>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let _ = handle(stream, state).await;
                });
            }
            Err(e) => eprintln!("workflow view: accept error: {e}"),
        }
    }
}

/// Read one request, route it, write one response, close. (No keep-alive — fine for
/// a local dev viewer; the browser opens fresh connections per fetch.)
async fn handle(mut stream: TcpStream, state: Arc<ServerState>) -> std::io::Result<()> {
    // Read until end of headers. Requests here are tiny (GET, or a POST with no
    // body — the write routes carry everything they need in the URL + the
    // X-Sema-View-Token header, never a body).
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let (method, path, token_header) = parse_request(&head);
    // Strip any query string before routing.
    let path = path.split('?').next().unwrap_or("/");

    let (status, content_type, body) = route(method, path, token_header.as_deref(), &state);
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await
}

/// Parse the request line's method + path, plus the `X-Sema-View-Token` header
/// (case-insensitive name, per RFC 7230) if present. Any other header is
/// ignored — this server has no use for them.
fn parse_request(head: &str) -> (&str, &str, Option<String>) {
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");

    let mut token = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("x-sema-view-token") {
                token = Some(value.trim().to_string());
            }
        }
    }
    (method, path, token)
}

fn not_found() -> JsonResponse {
    ("404 Not Found", "text/plain", b"no such run/file".to_vec())
}

fn forbidden() -> JsonResponse {
    (
        "403 Forbidden",
        "application/json",
        br#"{"error":"missing or invalid X-Sema-View-Token"}"#.to_vec(),
    )
}

/// Constant-time comparison for the write-route session-token check: a naive
/// `!=` bails out at the first mismatched byte, giving a timing attacker a
/// byte-at-a-time oracle to guess `state.token` with. `header` missing is
/// still a fast reject (there is no byte content to compare in constant
/// time against), but any present header of the RIGHT length is compared for
/// its full length regardless of where the mismatch is.
fn token_matches(header: Option<&str>, expected: &str) -> bool {
    let Some(given) = header else {
        return false;
    };
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn route(
    method: &str,
    path: &str,
    token_header: Option<&str>,
    state: &Arc<ServerState>,
) -> JsonResponse {
    if method == "GET" && path == "/" {
        let body = INDEX_HTML.replacen(VIEW_TOKEN_PLACEHOLDER, &state.token, 1);
        return ("200 OK", "text/html; charset=utf-8", body.into_bytes());
    }
    if method == "GET" && path == "/alpine.min.js" {
        return (
            "200 OK",
            "application/javascript; charset=utf-8",
            ALPINE_JS.into(),
        );
    }
    if method == "GET" && path == "/api/runs" {
        return (
            "200 OK",
            "application/json",
            list_runs(&state.run_dir).into_bytes(),
        );
    }
    // Additive cross-run index (SQLite projection): a rich runs list with status, agent
    // count, tokens, and NULL-aware cost. Leaves the per-run JSONL routes untouched.
    if method == "GET" && path == "/api/index/runs" {
        return (
            "200 OK",
            "application/json",
            index_runs_json(&state.run_dir),
        );
    }
    // /api/run/<id>/… — <id> a single safe segment.
    if let Some(rest) = path.strip_prefix("/api/run/") {
        let segs: Vec<&str> = rest.split('/').collect();
        match (method, segs.as_slice()) {
            // Read-only MCP auth status, derived server-side from the journal +
            // metadata manifest + this process's own in-memory flow state (never
            // the token store itself — see `auth`'s module doc).
            ("GET", [id, "auth"]) => {
                if is_safe_segment(id) {
                    let overrides = state.flow_snapshot(id);
                    return (
                        "200 OK",
                        "application/json",
                        auth::auth_status_json(&state.run_dir, id, &overrides),
                    );
                }
                not_found()
            }
            // Write routes: token required BEFORE any validation/side effect —
            // a wrong/missing token must never reveal whether a run or alias
            // exists, and must never start a flow.
            ("POST", [id, "auth", alias, action @ ("connect" | "forget")]) => {
                if !token_matches(token_header, &state.token) {
                    return forbidden();
                }
                match *action {
                    "connect" => connect::handle_connect(state, id, alias),
                    "forget" => connect::handle_forget(state, id, alias),
                    _ => unreachable!("action is exactly \"connect\" or \"forget\""),
                }
            }
            ("GET", [id, file]) => {
                let (ctype, ok) = match *file {
                    "events.jsonl" => ("application/x-ndjson", true),
                    "result.json" | "metadata.json" | "args.json" => ("application/json", true),
                    _ => ("", false),
                };
                if ok && is_safe_segment(id) {
                    if let Ok(bytes) = std::fs::read(state.run_dir.join(id).join(file)) {
                        return ("200 OK", ctype, bytes);
                    }
                }
                not_found()
            }
            _ => not_found(),
        }
    } else {
        ("404 Not Found", "text/plain", b"not found".to_vec())
    }
}

/// JSON array of run-ids: immediate child dirs of `run_dir` that contain an
/// `events.jsonl`, newest first (by mtime).
fn list_runs(run_dir: &Path) -> String {
    let mut runs: Vec<(std::time::SystemTime, String)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(run_dir) {
        for e in entries.flatten() {
            let p = e.path();
            let journal = p.join("events.jsonl");
            if journal.is_file() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    let mtime = journal
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    runs.push((mtime, name.to_string()));
                }
            }
        }
    }
    runs.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    let items: Vec<String> = runs
        .into_iter()
        .map(|(_, n)| format!("\"{}\"", n.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("[{}]", items.join(","))
}

/// The cross-run SQLite summary as a JSON array. Opens `<run-dir>/index.db` per request
/// (open-per-request is fine for a loopback dev tool — no shared-mutex concurrency),
/// lazily backfills every run, and serializes the rich summary. Degrades to `[]` on any
/// SQLite error so the endpoint never 500s the viewer.
fn index_runs_json(run_dir: &Path) -> Vec<u8> {
    let try_build = || -> rusqlite::Result<Vec<u8>> {
        let conn = ingest::open(&run_dir.join(sema_workflow::INDEX_DB))?;
        ingest::backfill_all(&conn, run_dir);
        let rows = ingest::runs_summary(&conn)?;
        Ok(serde_json::to_vec(&serde_json::Value::Array(rows)).unwrap_or_else(|_| b"[]".to_vec()))
    };
    try_build().unwrap_or_else(|_| b"[]".to_vec())
}

/// A run-id is a single directory segment: no separators, no `..`, non-empty.
fn is_safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains("..")
        && !s.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TOKEN: &str = "test-token-0123456789abcdef0123456789abcdef";

    fn test_state(dir: &Path) -> Arc<ServerState> {
        Arc::new(ServerState::new(
            dir.to_path_buf(),
            TEST_TOKEN.to_string(),
            None,
        ))
    }

    #[test]
    fn safe_segment_rejects_traversal() {
        assert!(is_safe_segment("content-live4"));
        assert!(is_safe_segment("wf_2026_123"));
        assert!(!is_safe_segment(""));
        assert!(!is_safe_segment(".."));
        assert!(!is_safe_segment("../etc"));
        assert!(!is_safe_segment("a/b"));
        assert!(!is_safe_segment("a\\b"));
        assert!(!is_safe_segment("x\0y"));
    }

    #[test]
    fn static_routes_resolve() {
        let dir = std::path::Path::new("/nonexistent-run-dir");
        let state = test_state(dir);
        let (s1, ct1, b1) = route("GET", "/", None, &state);
        assert_eq!(s1, "200 OK");
        assert!(ct1.starts_with("text/html"));
        assert!(!b1.is_empty());
        // The session token is substituted into the served HTML, and the raw
        // placeholder never leaks through.
        let text1 = String::from_utf8(b1).unwrap();
        assert!(text1.contains(TEST_TOKEN), "{text1}");
        assert!(!text1.contains("__SEMA_VIEW_TOKEN__"), "{text1}");

        let (s2, ct2, _) = route("GET", "/alpine.min.js", None, &state);
        assert_eq!(s2, "200 OK");
        assert!(ct2.contains("javascript"));

        // Unknown run dir → empty runs list, still valid JSON.
        let (s3, _, b3) = route("GET", "/api/runs", None, &state);
        assert_eq!(s3, "200 OK");
        assert_eq!(String::from_utf8(b3).unwrap(), "[]");

        // Traversal in the run id → 404, never reads outside run_dir.
        let (s4, _, _) = route("GET", "/api/run/../../etc/events.jsonl", None, &state);
        assert_eq!(s4, "404 Not Found");

        let (s5, _, _) = route("GET", "/nope", None, &state);
        assert_eq!(s5, "404 Not Found");
    }

    #[test]
    fn auth_route_resolves_and_rejects_traversal() {
        let dir = std::path::Path::new("/nonexistent-run-dir");
        let state = test_state(dir);
        // Unknown run → empty auth manifest, still valid JSON, never a 500.
        let (s1, ct1, b1) = route("GET", "/api/run/no-such-run/auth", None, &state);
        assert_eq!(s1, "200 OK");
        assert_eq!(ct1, "application/json");
        assert_eq!(String::from_utf8(b1).unwrap(), "[]");

        // Traversal in the run id → 404, same discipline as the other run routes.
        let (s2, _, _) = route("GET", "/api/run/../../etc/auth", None, &state);
        assert_eq!(s2, "404 Not Found");
    }

    // ── Task 10: write-route token hardening + validation (all return before
    // any `spawn_blocking`, so none of these need a tokio runtime) ──────────

    #[test]
    fn connect_without_token_is_403() {
        let dir = std::path::Path::new("/nonexistent-run-dir");
        let state = test_state(dir);
        let (s, ct, b) = route("POST", "/api/run/some-run/auth/asana/connect", None, &state);
        assert_eq!(s, "403 Forbidden");
        assert_eq!(ct, "application/json");
        assert!(String::from_utf8(b).unwrap().contains("X-Sema-View-Token"));
    }

    #[test]
    fn connect_with_wrong_token_is_403() {
        let dir = std::path::Path::new("/nonexistent-run-dir");
        let state = test_state(dir);
        let (s, _, _) = route(
            "POST",
            "/api/run/some-run/auth/asana/connect",
            Some("definitely-not-the-token"),
            &state,
        );
        assert_eq!(s, "403 Forbidden");
    }

    #[test]
    fn connect_with_same_length_wrong_token_is_403() {
        // Same length as TEST_TOKEN, differing only in the last byte — the
        // case a short-circuiting `==`/`!=` and the constant-time compare
        // must reject identically.
        let dir = std::path::Path::new("/nonexistent-run-dir");
        let state = test_state(dir);
        let mut wrong = TEST_TOKEN.to_string();
        wrong.pop();
        wrong.push('0');
        assert_eq!(wrong.len(), TEST_TOKEN.len());
        assert_ne!(wrong, TEST_TOKEN);
        let (s, _, _) = route(
            "POST",
            "/api/run/some-run/auth/asana/connect",
            Some(wrong.as_str()),
            &state,
        );
        assert_eq!(s, "403 Forbidden");
    }

    #[test]
    fn token_matches_is_constant_time_safe_and_correct() {
        assert!(token_matches(Some(TEST_TOKEN), TEST_TOKEN));
        assert!(!token_matches(None, TEST_TOKEN));
        assert!(!token_matches(Some(""), TEST_TOKEN));
        assert!(!token_matches(Some("short"), TEST_TOKEN));
        // Same length, last byte differs.
        let mut wrong = TEST_TOKEN.to_string();
        wrong.pop();
        wrong.push('0');
        assert!(!token_matches(Some(&wrong), TEST_TOKEN));
    }

    #[test]
    fn forget_without_token_is_403() {
        let dir = std::path::Path::new("/nonexistent-run-dir");
        let state = test_state(dir);
        let (s, _, _) = route("POST", "/api/run/some-run/auth/asana/forget", None, &state);
        assert_eq!(s, "403 Forbidden");
    }

    #[test]
    fn connect_with_correct_token_but_no_such_run_is_404_not_a_spawn() {
        // No metadata.json under this (nonexistent) run dir → 404 before ever
        // touching `spawn_blocking`, so this needs no tokio runtime at all.
        let dir = std::path::Path::new("/nonexistent-run-dir");
        let state = test_state(dir);
        let (s, _, _) = route(
            "POST",
            "/api/run/some-run/auth/asana/connect",
            Some(TEST_TOKEN),
            &state,
        );
        assert_eq!(s, "404 Not Found");
    }
}
