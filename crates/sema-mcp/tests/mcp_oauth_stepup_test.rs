//! Mid-session `403 insufficient_scope` step-up re-authorization, offline.
//!
//! The client starts with a narrow-scope token (`read`) and hits a challenge
//! demanding `read write`. `reauth_on_challenge` must re-authorize requesting the
//! *union* of scopes and persist the upgraded token. The scripted AS echoes the
//! scope it was asked for back into the issued token, so the assertions prove the
//! union was computed, requested, and stored.

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sema_mcp::oauth::login::reauth_on_challenge;
use sema_mcp::oauth::loopback::{BrowserOpener, LoopbackDriver};
use sema_mcp::oauth::store::{ClientInfo, FileStore, StoredCredentials, TokenSet, TokenStore};

const SERVER: &str = r#"
import json, hashlib, base64
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs, urlencode

PORT = None
codes = {}  # code -> {challenge, scope}

def b64url_nopad(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()

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
                               "scopes_supported": ["read", "write"]})
        if p.path == "/.well-known/oauth-authorization-server":
            return self._json({"issuer": self.base(),
                               "authorization_endpoint": self.base() + "/authorize",
                               "token_endpoint": self.base() + "/token",
                               "registration_endpoint": self.base() + "/register",
                               "code_challenge_methods_supported": ["S256"]})
        if p.path == "/authorize":
            q = parse_qs(p.query)
            code = "authcode-stepup"
            codes[code] = {"challenge": q.get("code_challenge", [""])[0],
                           "scope": q.get("scope", [""])[0]}
            loc = q.get("redirect_uri", [""])[0] + "?" + urlencode(
                {"code": code, "state": q.get("state", [""])[0]})
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
            return self._json({"client_id": "c1"}, code=201)
        if p.path == "/token":
            body = parse_qs(raw.decode())
            code = body.get("code", [""])[0]
            rec = codes.get(code, {})
            verifier = body.get("code_verifier", [""])[0]
            if b64url_nopad(hashlib.sha256(verifier.encode()).digest()) != rec.get("challenge"):
                return self._json({"error": "invalid_grant"}, code=400)
            # Echo the granted scope back into the token so the test can assert it.
            return self._json({"access_token": "stepped-up-token", "token_type": "Bearer",
                               "expires_in": 3600, "scope": rec.get("scope", "")})
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
        .expect("spawn python3 step-up server");
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
        "sema-mcp-stepup-{}-{}/auth.json",
        std::process::id(),
        n
    )))
}

#[tokio::test]
async fn test_403_insufficient_scope_step_up() {
    let (_server, port) = start_server();
    let url = format!("http://127.0.0.1:{port}/mcp");

    // Seed a prior narrow-scope login (`read` only), with a client already
    // registered so the step-up reuses it.
    let store = temp_store();
    store
        .save(&StoredCredentials {
            server_url: url.clone(),
            tokens: TokenSet {
                access_token: "old-narrow-token".to_string(),
                refresh_token: None,
                expires_at: None,
                scope: Some("read".to_string()),
            },
            client_info: Some(ClientInfo {
                client_id: "c1".to_string(),
                client_secret: None,
            }),
        })
        .unwrap();

    // The server now demands `read write`.
    let challenge = r#"Bearer error="insufficient_scope", scope="read write""#;
    let opener: BrowserOpener = Box::new(|u: &str| {
        reqwest::blocking::Client::new()
            .get(u)
            .send()
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let driver = LoopbackDriver::with_opener(Duration::from_secs(10), opener).unwrap();

    let token = reauth_on_challenge(
        &reqwest::Client::new(),
        &store,
        &url,
        Some(403),
        Some(challenge),
        None,
        &driver,
    )
    .await
    .expect("step-up should succeed")
    .expect("a 403 insufficient_scope must trigger re-auth");

    // A fresh token was obtained…
    assert_eq!(token, "stepped-up-token");
    assert_ne!(token, "old-narrow-token");
    // …the upgraded scope (union of read + read/write) was requested and stored…
    let stored = store.load(&url).expect("credentials persisted");
    assert_eq!(stored.tokens.scope.as_deref(), Some("read write"));
    assert_eq!(stored.tokens.access_token, "stepped-up-token");
}
