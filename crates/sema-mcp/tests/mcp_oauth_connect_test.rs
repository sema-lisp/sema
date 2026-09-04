//! OAuth-gated connect: the MCP endpoint answers `401` (with a
//! `WWW-Authenticate` challenge) until a valid bearer token is presented. This
//! exercises the whole connect→auth→retry path plus the "reconnect silently
//! from cached tokens" property — offline, no browser.
//!
//! The Rust side mirrors what `mcp/connect` does for a `:url` server (probe →
//! 401 → `ensure_access_token` → attach bearer → retry), but injects a
//! reqwest-blocking "browser" and a temp-file token store so it runs in CI. The
//! second connection uses an opener that hard-errors, so a green test proves the
//! cached token was reused without a fresh login.

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

use sema_mcp::oauth::login::{ensure_access_token, LoginConfig};
use sema_mcp::oauth::loopback::{BrowserOpener, LoopbackDriver};
use sema_mcp::oauth::store::FileStore;
use sema_mcp::{McpClient, McpHttpConfig};

const SERVER: &str = r#"
import json, hashlib, base64
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs, urlencode

PORT = None
codes = {}
ACCESS = "access-token-xyz"
REFRESH = "refresh-token-xyz"

def b64url_nopad(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()

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
                               "registration_endpoint": self.base() + "/register",
                               "code_challenge_methods_supported": ["S256"],
                               "grant_types_supported": ["authorization_code", "refresh_token"]})
        if p.path == "/authorize":
            q = parse_qs(p.query)
            redirect_uri = q.get("redirect_uri", [""])[0]
            state = q.get("state", [""])[0]
            code = "authcode-123"
            codes[code] = q.get("code_challenge", [""])[0]
            loc = redirect_uri + "?" + urlencode({"code": code, "state": state})
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
        if p.path == "/register":
            return self._json({"client_id": "dcr-client-1"}, code=201)
        if p.path == "/token":
            body = parse_qs(raw.decode())
            grant = body.get("grant_type", [""])[0]
            if grant == "authorization_code":
                verifier = body.get("code_verifier", [""])[0]
                code = body.get("code", [""])[0]
                if b64url_nopad(hashlib.sha256(verifier.encode()).digest()) != codes.get(code):
                    return self._json({"error": "invalid_grant"}, code=400)
                return self._json({"access_token": ACCESS, "refresh_token": REFRESH,
                                   "token_type": "Bearer", "expires_in": 3600, "scope": "mcp:tools"})
            return self._json({"error": "unsupported_grant_type"}, code=400)
        if p.path == "/mcp":
            auth = self.headers.get("Authorization", "")
            if auth != "Bearer " + ACCESS:
                # Challenge: point the client at the protected-resource metadata.
                self.send_response(401)
                self.send_header("WWW-Authenticate",
                                 'Bearer resource_metadata="%s/.well-known/oauth-protected-resource"' % self.base())
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
                return self._json({"jsonrpc": "2.0", "id": rid, "result": {
                    "protocolVersion": "2025-11-25", "capabilities": {},
                    "serverInfo": {"name": "gated-server", "version": "1.0"}}},
                    headers={"Mcp-Session-Id": "sess-1"})
            if method == "tools/list":
                return self._json({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
                    {"name": "ping", "description": "Ping",
                     "inputSchema": {"type": "object", "properties": {}}}]}})
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

fn start_server() -> (ServerGuard, u16) {
    let mut child = Command::new("python3")
        .args(["-c", SERVER])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn python3 gated server");
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

fn temp_store_path() -> std::path::PathBuf {
    sema_core::testing::unique_temp_dir("mcp-connect").join("auth.json")
}

/// Connect, and if the server challenges with a 401, authenticate via
/// `ensure_access_token` and retry — the same shape as the `mcp/connect`
/// builtin, but with an injected opener + store for CI.
async fn connect_with_auth(url: &str, store: &FileStore, opener: BrowserOpener) -> McpClient {
    let mut client = McpClient::connect_http(McpHttpConfig::new(url))
        .await
        .expect("prepare http client");
    if client.initialize().await.is_err() {
        let challenge = client
            .http_challenge()
            .expect("initialize failed for a reason other than 401");
        let parsed = sema_mcp::oauth::discovery::parse_www_authenticate(&challenge);
        let driver =
            LoopbackDriver::with_opener(Duration::from_secs(10), opener).expect("bind loopback");
        let config = LoginConfig {
            mcp_url: url,
            resource_metadata_url: parsed.resource_metadata.as_deref(),
            requested_scope: parsed.scope.as_deref(),
            preconfigured_client_id: None,
        };
        sema_mcp::ensure_crypto_provider();
        let token = ensure_access_token(&reqwest::Client::new(), store, &config, &driver)
            .await
            .expect("ensure_access_token");
        client.set_bearer_token(&token);
        client.initialize().await.expect("initialize after auth");
    }
    client
}

fn browser_opener() -> BrowserOpener {
    Box::new(|url: &str| {
        sema_mcp::ensure_crypto_provider();
        reqwest::blocking::Client::new()
            .get(url)
            .send()
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

#[tokio::test]
async fn test_oauth_gated_connect_then_cached_reconnect() {
    let (_server, port) = start_server();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let store = FileStore::new(temp_store_path());

    // First connect: 401 -> full login (browser opener drives the redirect) ->
    // token -> retry -> tools work.
    let mut client = connect_with_auth(&url, &store, browser_opener()).await;
    let tools = client.list_tools().await.expect("tools/list after auth");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "ping");
    client.close().await.ok();

    // Second connect against the SAME store: the cached token must be reused
    // without any login. The opener hard-errors, so if a login were attempted
    // the flow would fail — a green test proves the silent reconnect.
    let panicking_opener: BrowserOpener =
        Box::new(|_url: &str| Err("browser must not be opened on a cached reconnect".to_string()));
    let mut client2 = connect_with_auth(&url, &store, panicking_opener).await;
    let tools2 = client2.list_tools().await.expect("tools/list on reconnect");
    assert_eq!(tools2.len(), 1);
    client2.close().await.ok();
}
