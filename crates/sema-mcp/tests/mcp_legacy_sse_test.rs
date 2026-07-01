//! Deprecated 2024-11-05 HTTP+SSE transport. A threaded Python server holds the
//! GET SSE stream open (announcing the POST endpoint via the first `endpoint`
//! event) and pushes each JSON-RPC response onto that stream when a POST arrives
//! — exactly the two-endpoint shape a client must speak for backwards compat.

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};

use sema_mcp::{McpClient, McpHttpConfig};

const SERVER: &str = r#"
import json, queue
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

PORT = None
q = queue.Queue()

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        p = urlparse(self.path)
        if p.path == "/sse":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            self.wfile.write(b"event: endpoint\ndata: /messages\n\n")
            self.wfile.flush()
            while True:
                msg = q.get()
                if msg is None:
                    break
                self.wfile.write(("event: message\ndata: " + json.dumps(msg) + "\n\n").encode())
                self.wfile.flush()
            return
        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        p = urlparse(self.path)
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b""
        if p.path == "/messages":
            msg = json.loads(raw) if raw else {}
            method = msg.get("method")
            rid = msg.get("id")
            self.send_response(202)
            self.end_headers()
            if rid is None:
                return
            if method == "initialize":
                q.put({"jsonrpc": "2.0", "id": rid, "result": {
                    "protocolVersion": "2024-11-05", "capabilities": {},
                    "serverInfo": {"name": "legacy-server", "version": "1.0"}}})
            elif method == "tools/list":
                # Interleave a server->client notification and a server->client
                # request whose id COLLIDES with the client's request, before the
                # real response. A correct client skips both (they carry `method`)
                # and returns only the real result.
                q.put({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}})
                q.put({"jsonrpc": "2.0", "method": "ping", "id": rid})
                q.put({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
                    {"name": "legacy-echo", "description": "Echo",
                     "inputSchema": {"type": "object", "properties": {}}}]}})
            else:
                q.put({"jsonrpc": "2.0", "id": rid,
                       "error": {"code": -32601, "message": "Method not found"}})
            return
        self.send_response(404)
        self.end_headers()

srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
srv.daemon_threads = True
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
        .expect("spawn python3 legacy SSE server");
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
async fn test_legacy_http_sse_transport() {
    let (_server, port) = start_server();
    let url = format!("http://127.0.0.1:{port}/sse");

    let mut client = McpClient::connect_legacy_sse(McpHttpConfig::new(url))
        .await
        .expect("connect over legacy HTTP+SSE");

    let init = client
        .initialize()
        .await
        .expect("initialize over legacy transport");
    assert_eq!(init["serverInfo"]["name"], "legacy-server");

    let tools = client
        .list_tools()
        .await
        .expect("tools/list over legacy transport");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "legacy-echo");

    // A second request must correlate correctly on the shared stream too.
    let tools_again = client.list_tools().await.expect("second tools/list");
    assert_eq!(tools_again.len(), 1);

    client.close().await.ok();
}
