//! `sema workflow view` — a tiny web viewer over workflow run journals.
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
//! non-loopback host exposes the run directory's contents and MCP auth write
//! endpoints to the network; that is the operator's responsibility. Approval
//! signing is stricter: a viewer configured with a private approval authority
//! refuses to bind a non-loopback host.
//!
//! **Write-route hardening (plan §8):** MCP `connect`/`forget` and approval
//! decisions can cause external or durable changes, so "loopback + no auth"
//! alone is not enough for those routes. A malicious local page could otherwise
//! POST to them blind. At startup this server mints a random 32-hex session token
//! (`sema_mcp::random_hex_token`) and substitutes it into the served HTML in
//! place of the `__SEMA_VIEW_TOKEN__` placeholder (see `route`'s `"/"` case).
//! Every request must also carry a `Host` header for the configured loopback
//! listener. This blocks DNS rebinding through an attacker-controlled hostname.
//! Browser writes must be same-origin when `Origin` or Fetch Metadata is present.
//! Every write route requires `X-Sema-View-Token: <token>` matching exactly;
//! approval forms may carry the same token in a hidden form field so they still
//! work without JavaScript. Missing or wrong tokens get a `403` with no side
//! effects. The custom header forces a CORS preflight on cross-origin `fetch`
//! requests, which this server never answers (there is no
//! `Access-Control-Allow-Origin` handling); the unguessable hidden field gives
//! native forms equivalent protection. GET routes stay unauthenticated
//! (read-only, same trust model as before).
//!
//! No new crate dependency: a ~hand-rolled HTTP/1.1 handler over the `tokio` net/io
//! the binary already pulls in (the notebook uses axum; a handful of routes does
//! not need it).

use std::net::SocketAddr;

use sema_core::net::{format_host_port, is_loopback_host};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

mod approval;
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
const FAVICON_SVG: &[u8] = include_bytes!("workflow_view/favicon.svg");
/// Substituted for the real per-process session token at response time (see
/// the module doc's §8 note). Must match the placeholder literal embedded in
/// `workflow_view/index.html`'s `<script>`.
const VIEW_TOKEN_PLACEHOLDER: &str = "__SEMA_VIEW_TOKEN__";
const VIEW_NONCE_PLACEHOLDER: &str = "__SEMA_VIEW_NONCE__";
const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 64 * 1024;

/// Host-only approval authority made available to a loopback workflow viewer.
/// The private key is never serialized into the page or returned by an API.
#[derive(Clone)]
pub struct ApprovalAuthority {
    signing_key: sema_workflow::approval::ApprovalSigningKey,
    public_key: String,
    actor: String,
}

impl ApprovalAuthority {
    pub fn new(
        signing_key: sema_workflow::approval::ApprovalSigningKey,
        actor: String,
    ) -> std::io::Result<Self> {
        let public_key = signing_key.public_key_base64()?;
        Ok(Self {
            signing_key,
            public_key,
            actor,
        })
    }
}

/// Serve the viewer for the runs under `run_dir`, exiting the process on a bind
/// failure. Used by the standalone `sema workflow view` command.
pub async fn serve(run_dir: PathBuf, host: &str, port: u16) {
    serve_with_approval(run_dir, host, port, None).await;
}

/// Serve the viewer with an optional host-held approval authority.
pub async fn serve_with_approval(
    run_dir: PathBuf,
    host: &str,
    port: u16,
    approval_authority: Option<ApprovalAuthority>,
) {
    if let Err(e) = serve_result_with_approval(run_dir, host, port, true, approval_authority).await
    {
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
    serve_result_with_approval(run_dir, host, port, announce, None).await
}

/// Variant of [`serve_result`] that enables signed approval decisions. Private
/// authorities are deliberately limited to loopback binds because this viewer
/// has no user authentication layer.
pub async fn serve_result_with_approval(
    run_dir: PathBuf,
    host: &str,
    port: u16,
    announce: bool,
    approval_authority: Option<ApprovalAuthority>,
) -> std::io::Result<()> {
    if approval_authority.is_some() && !is_loopback_host(host) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "approval-enabled workflow viewer must bind to a loopback host",
        ));
    }
    let listener = TcpListener::bind((host, port)).await?;
    let listener_addr = listener.local_addr()?;
    let addr = format_host_port(host, listener_addr.port());
    if announce {
        println!("Sema workflow viewer:  http://{addr}");
        println!("  runs: {}", run_dir.display());
    }
    let state = Arc::new(ServerState::new(
        run_dir,
        sema_mcp::random_hex_token(),
        allowed_host_authorities(host, listener_addr),
        None,
        approval_authority,
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
    serve_test_with_approval(run_dir, opener, None).await
}

/// Test seam for approval-enabled viewer routes.
pub async fn serve_test_with_approval(
    run_dir: PathBuf,
    opener: Option<connect::TestOpenerFn>,
    approval_authority: Option<ApprovalAuthority>,
) -> std::io::Result<(std::net::SocketAddr, String)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let token = sema_mcp::random_hex_token();
    let state = Arc::new(ServerState::new(
        run_dir,
        token.clone(),
        vec![addr.to_string().to_ascii_lowercase()],
        opener,
        approval_authority,
    ));
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

/// Read one bounded request, route it, write one response, close. (No keep-alive —
/// fine for a local dev viewer; the browser opens fresh connections per fetch.)
async fn handle(mut stream: TcpStream, state: Arc<ServerState>) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return write_response(
                &mut stream,
                (
                    "400 Bad Request",
                    "text/plain",
                    b"incomplete request".to_vec(),
                ),
                &state.csp_nonce,
            )
            .await;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(offset) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = offset + 4;
            if header_end > MAX_HTTP_HEADER_BYTES {
                return request_headers_too_large(&mut stream, &state.csp_nonce).await;
            }
            break header_end;
        }
        if buf.len() > MAX_HTTP_HEADER_BYTES {
            return request_headers_too_large(&mut stream, &state.csp_nonce).await;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]);
    let parsed = match parse_request(&head) {
        Ok(parsed) => parsed,
        Err(RequestParseError::BadRequest(message)) => {
            return write_response(
                &mut stream,
                ("400 Bad Request", "text/plain", message.as_bytes().to_vec()),
                &state.csp_nonce,
            )
            .await;
        }
        Err(RequestParseError::PayloadTooLarge) => {
            return write_response(
                &mut stream,
                (
                    "413 Content Too Large",
                    "text/plain",
                    b"request body is too large".to_vec(),
                ),
                &state.csp_nonce,
            )
            .await;
        }
    };
    if let Some(response) = request_authority_error(&parsed, &state) {
        return write_response(&mut stream, response, &state.csp_nonce).await;
    }
    let request_end = header_end + parsed.content_length;
    while buf.len() < request_end {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return write_response(
                &mut stream,
                (
                    "400 Bad Request",
                    "text/plain",
                    b"incomplete request body".to_vec(),
                ),
                &state.csp_nonce,
            )
            .await;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = &buf[header_end..request_end];
    // Strip any query string before routing.
    let path = parsed.path.split('?').next().unwrap_or("/");

    let response = route(
        &parsed.method,
        path,
        parsed.token_header.as_deref(),
        body,
        &state,
    );
    write_response(&mut stream, response, &state.csp_nonce).await
}

async fn request_headers_too_large(stream: &mut TcpStream, nonce: &str) -> std::io::Result<()> {
    write_response(
        stream,
        (
            "431 Request Header Fields Too Large",
            "text/plain",
            b"request headers are too large".to_vec(),
        ),
        nonce,
    )
    .await
}

async fn write_response(
    stream: &mut TcpStream,
    response: JsonResponse,
    nonce: &str,
) -> std::io::Result<()> {
    let (status, content_type, body) = response;
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'self'; script-src 'self' 'nonce-{nonce}'; style-src 'self' 'unsafe-inline'; img-src 'self'; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'; object-src 'none'\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Resource-Policy: same-origin\r\nPermissions-Policy: camera=(), geolocation=(), microphone=()\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\n\r\n",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await
}

struct ParsedRequest {
    method: String,
    path: String,
    host_header: Option<String>,
    origin_header: Option<String>,
    fetch_site_header: Option<String>,
    token_header: Option<String>,
    content_length: usize,
}

#[derive(Debug)]
enum RequestParseError {
    BadRequest(&'static str),
    PayloadTooLarge,
}

/// Parse the request line plus the security and framing headers the viewer uses. Ambiguous
/// framing is rejected because write routes accept request bodies.
fn parse_request(head: &str) -> Result<ParsedRequest, RequestParseError> {
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or(RequestParseError::BadRequest("missing HTTP method"))?;
    let path = parts
        .next()
        .ok_or(RequestParseError::BadRequest("missing HTTP path"))?;
    let version = parts
        .next()
        .ok_or(RequestParseError::BadRequest("missing HTTP version"))?;
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") || parts.next().is_some() {
        return Err(RequestParseError::BadRequest("invalid HTTP request line"));
    }

    let mut token = None;
    let mut host = None;
    let mut origin = None;
    let mut fetch_site = None;
    let mut content_length = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            if name.eq_ignore_ascii_case("host") {
                if host.is_some() {
                    return Err(RequestParseError::BadRequest("duplicate Host header"));
                }
                host = Some(value.trim().to_string());
            } else if name.eq_ignore_ascii_case("origin") {
                if origin.is_some() {
                    return Err(RequestParseError::BadRequest("duplicate Origin header"));
                }
                origin = Some(value.trim().to_string());
            } else if name.eq_ignore_ascii_case("sec-fetch-site") {
                if fetch_site.is_some() {
                    return Err(RequestParseError::BadRequest(
                        "duplicate Sec-Fetch-Site header",
                    ));
                }
                fetch_site = Some(value.trim().to_string());
            } else if name.eq_ignore_ascii_case("x-sema-view-token") {
                if token.is_some() {
                    return Err(RequestParseError::BadRequest(
                        "duplicate X-Sema-View-Token header",
                    ));
                }
                token = Some(value.trim().to_string());
            } else if name.eq_ignore_ascii_case("content-length") {
                if content_length.is_some() {
                    return Err(RequestParseError::BadRequest(
                        "duplicate Content-Length header",
                    ));
                }
                let parsed = value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| RequestParseError::BadRequest("invalid Content-Length header"))?;
                if parsed > MAX_HTTP_BODY_BYTES {
                    return Err(RequestParseError::PayloadTooLarge);
                }
                content_length = Some(parsed);
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                return Err(RequestParseError::BadRequest(
                    "Transfer-Encoding is not supported",
                ));
            }
        }
    }
    Ok(ParsedRequest {
        method: method.to_string(),
        path: path.to_string(),
        host_header: host,
        origin_header: origin,
        fetch_site_header: fetch_site,
        token_header: token,
        content_length: content_length.unwrap_or(0),
    })
}

fn request_authority_error(request: &ParsedRequest, state: &ServerState) -> Option<JsonResponse> {
    let Some(host) = request.host_header.as_deref() else {
        return Some((
            "400 Bad Request",
            "text/plain",
            b"missing Host header".to_vec(),
        ));
    };
    if !state.allowed_hosts.is_empty()
        && !state
            .allowed_hosts
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(host))
    {
        return Some((
            "421 Misdirected Request",
            "text/plain",
            b"request Host does not match the loopback viewer".to_vec(),
        ));
    }
    if request.method != "POST" {
        return None;
    }
    if let Some(origin) = request.origin_header.as_deref() {
        let expected = format!("http://{host}");
        if !origin.eq_ignore_ascii_case(&expected) {
            return Some(forbidden());
        }
    }
    if request
        .fetch_site_header
        .as_deref()
        .is_some_and(|site| !matches!(site, "same-origin" | "none"))
    {
        return Some(forbidden());
    }
    None
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
    body: &[u8],
    state: &Arc<ServerState>,
) -> JsonResponse {
    if method == "GET" && path == "/" {
        let body = INDEX_HTML
            .replacen(VIEW_TOKEN_PLACEHOLDER, &state.token, 1)
            .replace(VIEW_NONCE_PLACEHOLDER, &state.csp_nonce);
        return ("200 OK", "text/html; charset=utf-8", body.into_bytes());
    }
    if method == "GET" && path == "/alpine.min.js" {
        return (
            "200 OK",
            "application/javascript; charset=utf-8",
            ALPINE_JS.into(),
        );
    }
    if method == "GET" && path == "/favicon.svg" {
        return ("200 OK", "image/svg+xml", FAVICON_SVG.to_vec());
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
            ("GET", [id, "approvals"]) => {
                if is_safe_segment(id) {
                    return approval::list_json(state, id);
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
            ("POST", [id, "approvals", approval_id, "decision"]) => {
                let body_token = unique_form_field(body, "view_token");
                if !token_matches(token_header, &state.token)
                    && !token_matches(body_token.as_deref(), &state.token)
                {
                    return forbidden();
                }
                if !is_safe_segment(id) || !is_safe_segment(approval_id) {
                    return not_found();
                }
                approval::handle_decision(state, id, approval_id, body)
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

fn unique_form_field(body: &[u8], wanted: &str) -> Option<String> {
    let mut found = None;
    for (name, value) in url::form_urlencoded::parse(body) {
        if name == wanted {
            if found.is_some() {
                return None;
            }
            found = Some(value.into_owned());
        }
    }
    found
}

fn allowed_host_authorities(host: &str, listener_addr: SocketAddr) -> Vec<String> {
    if !is_loopback_host(host) {
        return Vec::new();
    }
    let mut authorities = vec![format_host_port(host, listener_addr.port()).to_ascii_lowercase()];
    let bound = listener_addr.to_string().to_ascii_lowercase();
    if !authorities.contains(&bound) {
        authorities.push(bound);
    }
    authorities
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
            vec!["127.0.0.1:8899".to_string()],
            None,
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
        let (s1, ct1, b1) = route("GET", "/", None, b"", &state);
        assert_eq!(s1, "200 OK");
        assert!(ct1.starts_with("text/html"));
        assert!(!b1.is_empty());
        // The session token is substituted into the served HTML, and the raw
        // placeholder never leaks through.
        let text1 = String::from_utf8(b1).unwrap();
        assert!(text1.contains(TEST_TOKEN), "{text1}");
        assert!(!text1.contains("__SEMA_VIEW_TOKEN__"), "{text1}");
        assert!(!text1.contains("__SEMA_VIEW_NONCE__"), "{text1}");

        let (s2, ct2, _) = route("GET", "/alpine.min.js", None, b"", &state);
        assert_eq!(s2, "200 OK");
        assert!(ct2.contains("javascript"));

        let (favicon_status, favicon_type, favicon) =
            route("GET", "/favicon.svg", None, b"", &state);
        assert_eq!(favicon_status, "200 OK");
        assert_eq!(favicon_type, "image/svg+xml");
        assert_eq!(favicon, FAVICON_SVG);

        // Unknown run dir → empty runs list, still valid JSON.
        let (s3, _, b3) = route("GET", "/api/runs", None, b"", &state);
        assert_eq!(s3, "200 OK");
        assert_eq!(String::from_utf8(b3).unwrap(), "[]");

        // Traversal in the run id → 404, never reads outside run_dir.
        let (s4, _, _) = route("GET", "/api/run/../../etc/events.jsonl", None, b"", &state);
        assert_eq!(s4, "404 Not Found");

        let (s5, _, _) = route("GET", "/nope", None, b"", &state);
        assert_eq!(s5, "404 Not Found");
    }

    #[test]
    fn auth_route_resolves_and_rejects_traversal() {
        let dir = std::path::Path::new("/nonexistent-run-dir");
        let state = test_state(dir);
        // Unknown run → empty auth manifest, still valid JSON, never a 500.
        let (s1, ct1, b1) = route("GET", "/api/run/no-such-run/auth", None, b"", &state);
        assert_eq!(s1, "200 OK");
        assert_eq!(ct1, "application/json");
        assert_eq!(String::from_utf8(b1).unwrap(), "[]");

        // Traversal in the run id → 404, same discipline as the other run routes.
        let (s2, _, _) = route("GET", "/api/run/../../etc/auth", None, b"", &state);
        assert_eq!(s2, "404 Not Found");
    }

    // ── Task 10: write-route token hardening + validation (all return before
    // any `spawn_blocking`, so none of these need a tokio runtime) ──────────

    #[test]
    fn connect_without_token_is_403() {
        let dir = std::path::Path::new("/nonexistent-run-dir");
        let state = test_state(dir);
        let (s, ct, b) = route(
            "POST",
            "/api/run/some-run/auth/asana/connect",
            None,
            b"",
            &state,
        );
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
            b"",
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
            b"",
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
        let (s, _, _) = route(
            "POST",
            "/api/run/some-run/auth/asana/forget",
            None,
            b"",
            &state,
        );
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
            b"",
            &state,
        );
        assert_eq!(s, "404 Not Found");
    }

    #[test]
    fn request_parser_rejects_ambiguous_or_oversized_bodies() {
        assert!(matches!(
            parse_request("POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\n"),
            Err(RequestParseError::BadRequest(_))
        ));
        assert!(matches!(
            parse_request("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Err(RequestParseError::BadRequest(_))
        ));
        let oversized = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_HTTP_BODY_BYTES + 1
        );
        assert!(matches!(
            parse_request(&oversized),
            Err(RequestParseError::PayloadTooLarge)
        ));
        assert!(matches!(
            parse_request("GET / HTTP/2.0\r\n\r\n"),
            Err(RequestParseError::BadRequest(_))
        ));
        assert!(matches!(
            parse_request("GET / HTTP/1.1 trailing\r\n\r\n"),
            Err(RequestParseError::BadRequest(_))
        ));
        assert!(matches!(
            parse_request("GET / HTTP/1.1\r\nHost: 127.0.0.1:8899\r\nHost: evil.test\r\n\r\n"),
            Err(RequestParseError::BadRequest(_))
        ));
    }

    #[test]
    fn request_authority_rejects_dns_rebinding_and_cross_origin_writes() {
        let state = test_state(Path::new("/nonexistent-run-dir"));
        let parse = |request| parse_request(request).expect("valid request syntax");

        let missing = parse("GET / HTTP/1.1\r\n\r\n");
        assert!(matches!(
            request_authority_error(&missing, &state),
            Some(("400 Bad Request", _, _))
        ));

        let rebound = parse("GET / HTTP/1.1\r\nHost: attacker.example:8899\r\n\r\n");
        assert!(matches!(
            request_authority_error(&rebound, &state),
            Some(("421 Misdirected Request", _, _))
        ));

        let cross_origin = parse(
            "POST /api/run/r/approvals/a/decision HTTP/1.1\r\nHost: 127.0.0.1:8899\r\nOrigin: https://attacker.example\r\n\r\n",
        );
        assert!(matches!(
            request_authority_error(&cross_origin, &state),
            Some(("403 Forbidden", _, _))
        ));

        let cross_site = parse(
            "POST /api/run/r/approvals/a/decision HTTP/1.1\r\nHost: 127.0.0.1:8899\r\nSec-Fetch-Site: cross-site\r\n\r\n",
        );
        assert!(matches!(
            request_authority_error(&cross_site, &state),
            Some(("403 Forbidden", _, _))
        ));

        let same_origin = parse(
            "POST /api/run/r/approvals/a/decision HTTP/1.1\r\nHost: 127.0.0.1:8899\r\nOrigin: http://127.0.0.1:8899\r\nSec-Fetch-Site: same-origin\r\n\r\n",
        );
        assert!(request_authority_error(&same_origin, &state).is_none());

        let cli =
            parse("POST /api/run/r/approvals/a/decision HTTP/1.1\r\nHost: 127.0.0.1:8899\r\n\r\n");
        assert!(request_authority_error(&cli, &state).is_none());
    }

    #[tokio::test]
    async fn server_rejects_a_terminated_header_over_the_limit() {
        let (addr, _) = serve_test(PathBuf::from("unused"), None).await.unwrap();
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let padding = "a".repeat(MAX_HTTP_HEADER_BYTES);
        let request = format!("GET / HTTP/1.1\r\nX-Padding: {padding}\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();

        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 431 Request Header Fields Too Large\r\n"));
    }

    #[tokio::test]
    async fn server_rejects_rebound_host_and_emits_browser_security_headers() {
        let (addr, token) = serve_test(PathBuf::from("unused"), None).await.unwrap();

        let mut rebound = TcpStream::connect(addr).await.unwrap();
        rebound
            .write_all(b"GET / HTTP/1.1\r\nHost: attacker.example\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        rebound.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 421 Misdirected Request\r\n"));
        assert!(!String::from_utf8_lossy(&response).contains(&token));

        let mut valid = TcpStream::connect(addr).await.unwrap();
        let request = format!("GET / HTTP/1.1\r\nHost: {addr}\r\n\r\n");
        valid.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        valid.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("Content-Security-Policy:"), "{response}");
        assert!(response.contains("frame-ancestors 'none'"), "{response}");
        assert!(response.contains("X-Frame-Options: DENY"), "{response}");
        assert!(response.contains("nonce=\""), "{response}");
        assert!(
            !response.contains(&format!("nonce=\"{token}\"")),
            "{response}"
        );
    }

    #[test]
    fn loopback_host_and_ipv6_url_formatting_are_portable() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.42.0.9"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("localhost"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.0.2.1"));
        assert_eq!(format_host_port("::1", 8899), "[::1]:8899");
        assert_eq!(format_host_port("127.0.0.1", 8899), "127.0.0.1:8899");
    }

    #[tokio::test]
    async fn approval_authority_refuses_non_loopback_bind() {
        let authority = ApprovalAuthority::new(
            sema_workflow::approval::ApprovalSigningKey::generate().unwrap(),
            "reviewer".to_string(),
        )
        .unwrap();
        let error = serve_result_with_approval(
            PathBuf::from("unused"),
            "0.0.0.0",
            0,
            false,
            Some(authority),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }
}
