//! Device-authorization grant (RFC 8628) end-to-end, offline. The scripted AS
//! advertises a device endpoint, returns `authorization_pending` on the first
//! poll and tokens on the second, and the client drives `device_login` to
//! completion — proving the poll/pending loop and the code display.

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use sema_mcp::oauth::device::device_login;
use sema_mcp::oauth::login::LoginConfig;

const SERVER: &str = r#"
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

PORT = None
polls = {"n": 0}

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
                               "scopes_supported": ["mcp:tools"]})
        if p.path == "/.well-known/oauth-authorization-server":
            return self._json({"issuer": self.base(),
                               "authorization_endpoint": self.base() + "/authorize",
                               "token_endpoint": self.base() + "/token",
                               "registration_endpoint": self.base() + "/register",
                               "device_authorization_endpoint": self.base() + "/device",
                               "code_challenge_methods_supported": ["S256"],
                               "grant_types_supported": ["authorization_code",
                                                         "urn:ietf:params:oauth:grant-type:device_code"]})
        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        p = urlparse(self.path)
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b""
        if p.path == "/register":
            return self._json({"client_id": "dcr-device-client"}, code=201)
        if p.path == "/device":
            return self._json({"device_code": "dev-code-1", "user_code": "WXYZ-1234",
                               "verification_uri": self.base() + "/activate",
                               "expires_in": 300, "interval": 0})
        if p.path == "/token":
            body = parse_qs(raw.decode())
            grant = body.get("grant_type", [""])[0]
            if grant == "urn:ietf:params:oauth:grant-type:device_code":
                assert body.get("resource", [""])[0] == self.base() + "/mcp"
                polls["n"] += 1
                if polls["n"] < 2:
                    return self._json({"error": "authorization_pending"}, code=400)
                return self._json({"access_token": "device-access", "refresh_token": "device-refresh",
                                   "token_type": "Bearer", "expires_in": 3600, "scope": "mcp:tools"})
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
        .expect("spawn python3 device server");
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

#[tokio::test]
async fn test_device_flow_end_to_end() {
    let (_server, port) = start_server();
    let mcp_url = format!("http://127.0.0.1:{port}/mcp");
    let http = reqwest::Client::new();

    let shown = Arc::new(Mutex::new(None));
    let shown_ref = Arc::clone(&shown);
    let display = move |device: &sema_mcp::oauth::device::DeviceAuthorization| {
        *shown_ref.lock().unwrap() =
            Some((device.user_code.clone(), device.verification_uri.clone()));
    };

    let config = LoginConfig {
        mcp_url: &mcp_url,
        resource_metadata_url: None,
        requested_scope: None,
        preconfigured_client_id: None,
    };

    let creds = device_login(&http, &config, None, &display)
        .await
        .expect("device login should complete");

    assert_eq!(creds.tokens.access_token, "device-access");
    assert_eq!(
        creds.tokens.refresh_token.as_deref(),
        Some("device-refresh")
    );
    assert_eq!(
        creds.client_info.expect("DCR client").client_id,
        "dcr-device-client"
    );
    let (user_code, uri) = shown.lock().unwrap().clone().expect("user code displayed");
    assert_eq!(user_code, "WXYZ-1234");
    assert!(uri.ends_with("/activate"));
}
