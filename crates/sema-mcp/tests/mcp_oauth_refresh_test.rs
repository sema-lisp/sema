//! Mid-session `401` (expired token) → refresh, offline. A seeded, expired token
//! with a refresh token must be refreshed (NOT re-logged-in via the browser) and
//! the rotated refresh token persisted. The scripted AS only serves the
//! refresh-token grant; the driver's opener hard-errors, so a green test proves
//! the refresh path ran without opening a browser.

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sema_mcp::oauth::login::reauth_on_challenge;
use sema_mcp::oauth::loopback::{BrowserOpener, LoopbackDriver};
use sema_mcp::oauth::store::{ClientInfo, FileStore, StoredCredentials, TokenSet, TokenStore};

const SERVER: &str = r#"
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

PORT = None

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def base(self):
        return "http://127.0.0.1:%d" % PORT

    def _json(self, obj, code=200):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        p = urlparse(self.path)
        if p.path == "/.well-known/oauth-protected-resource":
            return self._json({"resource": self.base() + "/mcp",
                               "authorization_servers": [self.base()],
                               "scopes_supported": ["read"]})
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
        if p.path == "/token":
            body = parse_qs(raw.decode())
            if body.get("grant_type", [""])[0] == "refresh_token":
                assert body.get("refresh_token", [""])[0] == "r1"
                assert body.get("resource", [""])[0] == self.base() + "/mcp"
                return self._json({"access_token": "refreshed-access", "refresh_token": "r2",
                                   "token_type": "Bearer", "expires_in": 3600, "scope": "read"})
            return self._json({"error": "unsupported_grant_type"}, code=400)
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
        .expect("spawn python3 refresh server");
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

fn temp_store() -> FileStore {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    FileStore::new(std::env::temp_dir().join(format!(
        "sema-mcp-refresh-{}-{}/auth.json",
        std::process::id(),
        n
    )))
}

#[tokio::test]
async fn test_401_mid_session_refresh() {
    let (_server, port) = start_server();
    let url = format!("http://127.0.0.1:{port}/mcp");

    let store = temp_store();
    store
        .save(&StoredCredentials {
            server_url: url.clone(),
            tokens: TokenSet {
                access_token: "old-expired".to_string(),
                refresh_token: Some("r1".to_string()),
                expires_at: Some(1), // long expired
                scope: Some("read".to_string()),
            },
            client_info: Some(ClientInfo {
                client_id: "c1".to_string(),
                client_secret: None,
            }),
        })
        .unwrap();

    // The browser MUST NOT open — a 401 with a refresh token refreshes silently.
    let opener: BrowserOpener =
        Box::new(|_url: &str| Err("browser must not open on a refresh".to_string()));
    let driver = LoopbackDriver::with_opener(Duration::from_secs(5), opener).unwrap();

    let token = reauth_on_challenge(
        &reqwest::Client::new(),
        &store,
        &url,
        Some(401),
        None,
        None,
        &driver,
    )
    .await
    .expect("refresh should succeed")
    .expect("a 401 with a refresh token must re-authorize");

    assert_eq!(token, "refreshed-access");
    let stored = store.load(&url).expect("credentials persisted");
    assert_eq!(stored.tokens.access_token, "refreshed-access");
    // The rotated refresh token must be persisted (OAuth 2.1 rotation).
    assert_eq!(stored.tokens.refresh_token.as_deref(), Some("r2"));
}
