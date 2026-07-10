//! `connect_from_config`'s non-interactive precursor: the workflow runtime
//! must be able to probe a declared MCP server without ever popping a browser
//! mid-run (`docs/plans/2026-06-24-workflow-mcp-auth.md` §3). A server that
//! answers every request with a `401` + `WWW-Authenticate` challenge is the
//! oracle: with `interactive_auth: false`, the connect must fail with
//! `ConnectFailure::NeedsAuth` instead of chasing the challenge.
//!
//! This server deliberately implements NOTHING beyond the `401` on `/mcp` —
//! no `/.well-known/oauth-protected-resource`, no `/authorize`, no `/token`.
//! If the implementation ever regressed to calling the interactive login path
//! for a non-interactive connection, discovery would 404 and the test would
//! see `ConnectFailure::Failed`, not `NeedsAuth` — so the assertion below
//! also discriminates against that regression, on top of the "by
//! construction" guarantee that `connect_http`'s `!opts.interactive_auth`
//! branch returns before `obtain_access_token` (and therefore the browser
//! opener / loopback listener it drives) is ever referenced.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};

use sema_core::Value;
use sema_mcp::builtins::{connect_from_config, ConnectFailure, ConnectOpts};

const SERVER: &str = r#"
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

PORT = None

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        p = urlparse(self.path)
        if p.path == "/mcp":
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Bearer realm="mcp"')
            self.end_headers()
            return
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
        .expect("spawn python3 401-challenging server");
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

#[test]
fn non_interactive_connect_against_401_challenge_yields_needs_auth() {
    let (_server, port) = start_server();
    let url = format!("http://127.0.0.1:{port}/mcp");

    let opts = ConnectOpts {
        interactive_auth: false,
        allowed_tools: None,
    };
    let err = connect_from_config(&http_config(&url), opts)
        .expect_err("a 401 challenge with interactive_auth: false must fail");
    match err {
        ConnectFailure::NeedsAuth { url: got } => assert_eq!(got, url),
        ConnectFailure::Failed(msg) => {
            panic!("expected NeedsAuth (no interactive attempt), got Failed({msg})")
        }
    }
}
