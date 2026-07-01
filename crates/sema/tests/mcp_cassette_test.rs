//! Acceptance gate for MCP-call cassette record/replay (M5). A stdio server
//! whose `count` tool returns an ever-incrementing `call-N` is the oracle: after
//! recording one call (`call-1`), a **replay** must return `call-1` again — if it
//! had actually hit the server the counter would have advanced to `call-2`. A
//! final cassette-free call proves the server *would* have returned `call-2`, so
//! the replayed value provably came from the tape, not the network.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use sema::Interpreter;
use sema_llm::builtins::{install_cassette, take_cassette};
use sema_llm::cassette::{Cassette, CassetteMode};

const SERVER: &str = r#"
import json, sys
initialized = False
counter = 0
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    r = json.loads(line); method = r.get("method"); rid = r.get("id")
    if rid is None:
        if method == "notifications/initialized":
            initialized = True
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "serverInfo": {"name": "counter", "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
            {"name": "count", "description": "increment",
             "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        counter += 1
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "content": [{"type": "text", "text": "call-%d" % counter}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "no"}})
"#;

fn connect_expr() -> String {
    let encoded = serde_json::to_string(SERVER).unwrap();
    format!(r#"(define server (mcp/connect {{:command "python3" :args ["-c" {encoded}]}}))"#)
}

fn tape_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "sema-mcp-cassette-{}-{}/tape.ndjson",
        std::process::id(),
        n
    ))
}

#[test]
fn mcp_call_records_then_replays_without_hitting_the_server() {
    let tape = tape_path();

    // `Interpreter::new()` registers the MCP builtins + the cassette hook and
    // resets runtime state — so install the cassette AFTER building it.
    let interp = Interpreter::new();
    interp.eval_str(&connect_expr()).expect("connect");

    // --- Record: the real call runs and is taped. ---
    install_cassette(Cassette::load(tape.clone(), CassetteMode::Record));
    let r1 = interp
        .eval_str(r#"(mcp/call server "count" {})"#)
        .expect("record call");
    assert_eq!(r1.as_str(), Some("call-1"));
    take_cassette()
        .expect("cassette installed")
        .save()
        .expect("save tape");

    // --- Replay: the server is still alive, but the call must be served from
    //     the tape (so the counter stays at 1). ---
    install_cassette(Cassette::load(tape, CassetteMode::Replay));
    let r2 = interp
        .eval_str(r#"(mcp/call server "count" {})"#)
        .expect("replay call");
    assert_eq!(
        r2.as_str(),
        Some("call-1"),
        "replay must return the recorded value, not re-hit the server"
    );

    // --- Proof: drop the cassette and call for real → the server advances to
    //     call-2, confirming the replay above did NOT touch it. ---
    take_cassette();
    let r3 = interp
        .eval_str(r#"(mcp/call server "count" {})"#)
        .expect("live call");
    assert_eq!(r3.as_str(), Some("call-2"));

    interp.eval_str("(mcp/close server)").ok();
}
