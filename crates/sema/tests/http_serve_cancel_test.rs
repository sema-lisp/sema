//! SRV-1 synthetic-level cancellation gate: a client disconnect (modeled here
//! as an explicit `RuntimeCommandHandle::cancel_root`, the same signal a
//! process shutdown or a dropped connection ultimately delivers) mid-handler
//! must tear the in-flight per-connection handler task down cleanly — no
//! orphaned task, no hang.
//!
//! This is IN-PROCESS (drives the `Interpreter`/`Runtime` host API directly,
//! not a subprocess), so it can assert `runtime_live_task_count() == 0` as the
//! leak oracle — a subprocess test can only observe "did it hang", not "did
//! the runtime actually reap everything". `http_serve_concurrent_test.rs`
//! covers the real subprocess + real TCP client shape.

#![cfg(not(target_arch = "wasm32"))]

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::thread;
use std::time::{Duration, Instant};

use sema_eval::Interpreter;
use sema_vm::runtime::RootOptions;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local_addr").port()
}

fn wait_until_listening(port: u16, deadline: Duration) -> bool {
    let addr = format!("127.0.0.1:{port}")
        .to_socket_addrs()
        .expect("resolve loopback")
        .next()
        .expect("one address");
    let start = Instant::now();
    while start.elapsed() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Fire a bare HTTP GET and immediately drop the connection without reading
/// the response — a client hang-up while the handler is still in flight.
fn fire_and_disconnect(port: u16) {
    let addr = format!("127.0.0.1:{port}")
        .to_socket_addrs()
        .expect("resolve loopback")
        .next()
        .expect("one address");
    if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
        let _ = stream.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        // Drop immediately — don't read the response. The handler is parked
        // in `async/sleep` for 5s at this point, far longer than we wait.
    }
}

/// Cancelling the `http/serve` root while a handler task is parked mid-`async/
/// sleep` must reap BOTH the accept-loop root task AND the spawned handler
/// task — `runtime_live_task_count()` returns to 0, proving no orphan.
#[test]
fn cancel_during_slow_handler_reaps_the_handler_task_no_leak() {
    let interp = Interpreter::new();
    let port = free_port();
    let program = format!(
        r#"(http/serve (fn (req) (begin (async/sleep 5000) (http/text "late"))) {{:port {port}}})"#
    );
    let handle = interp
        .submit_str(&program, RootOptions::default())
        .expect("http/serve submits as a root");
    let root_id = handle.id();
    let cmd = interp.command_handle();

    // Canceller thread: waits for the accept loop to bind, fires one request
    // (spawning + parking the per-connection handler task in its 5s sleep),
    // gives it a moment to actually park, then cancels the root — modeling a
    // process shutdown / client disconnect while the handler is in flight.
    // Only `cmd` (Send + Sync) and plain data cross the thread boundary;
    // `interp` itself never leaves this (main) thread.
    let canceller = thread::spawn(move || {
        assert!(
            wait_until_listening(port, Duration::from_secs(5)),
            "server never bound"
        );
        fire_and_disconnect(port);
        thread::sleep(Duration::from_millis(250));
        cmd.cancel_root(root_id)
    });

    // Blocks on THIS thread until the root settles — driving the runtime
    // (accepting the connection, spawning the handler task, parking it on
    // the sleep) the whole time, and unblocking promptly once the canceller
    // thread's `cancel_root` rides the same completion inbox.
    let result = interp.drive_until_settled(&handle);

    let cancelled = canceller.join().expect("canceller thread completes");
    assert!(
        cancelled,
        "cancel_root must accept the live http/serve root"
    );
    assert!(
        result.is_err(),
        "a cancelled http/serve root must settle as an error, not a value"
    );
    assert_eq!(
        interp.runtime_live_task_count(),
        0,
        "cancelling the server root must reap the in-flight handler task too — no leak"
    );
}

/// SRV-1 piece c: cancelling `http/serve` while a WebSocket handler is parked
/// IDLE in `(:recv conn)` (`ws/recv`'s cooperative `ServerWsRecvProbe` path,
/// `crates/sema-stdlib/src/server.rs`) must reap the connection's handler
/// task cleanly too — no orphan, no hang. This is the cancellation half of
/// the piece-c contract; `idle_websocket_does_not_block_plain_request`
/// (`http_serve_concurrent_test.rs`) covers the liveness half (a sibling
/// request isn't blocked). Using a real `tungstenite` WS client so the
/// connection genuinely upgrades and the handler genuinely parks in
/// `ws/recv` — a bare TCP connect/disconnect (like `fire_and_disconnect`
/// above) never reaches that code path.
#[test]
fn cancel_during_idle_websocket_recv_reaps_the_handler_task_no_leak() {
    let interp = Interpreter::new();
    let port = free_port();
    let program = format!(
        r#"(http/serve
             (fn (req)
               (if (= (:path req) "/ws")
                   (http/websocket (fn (conn) ((:recv conn))))
                   (http/text "unused")))
             {{:port {port}}})"#
    );
    let handle = interp
        .submit_str(&program, RootOptions::default())
        .expect("http/serve submits as a root");
    let root_id = handle.id();
    let cmd = interp.command_handle();

    // Canceller thread: waits for the accept loop to bind, opens a real WS
    // connection (parking the per-connection handler task in `ws/recv`),
    // gives it a moment to actually park, then cancels the root — modeling a
    // process shutdown / client disconnect while the WS handler is idling.
    let canceller = thread::spawn(move || {
        assert!(
            wait_until_listening(port, Duration::from_secs(5)),
            "server never bound"
        );
        let ws_url = format!("ws://127.0.0.1:{port}/ws");
        let ws = tungstenite::connect(&ws_url);
        assert!(ws.is_ok(), "WS upgrade failed: {:?}", ws.err());
        let (socket, _resp) = ws.unwrap();
        thread::sleep(Duration::from_millis(250));
        let cancelled = cmd.cancel_root(root_id);
        // Keep the socket alive until after cancellation is issued so the
        // handler is genuinely still parked in `ws/recv`, not merely
        // disconnected client-side first.
        drop(socket);
        cancelled
    });

    let result = interp.drive_until_settled(&handle);

    let cancelled = canceller.join().expect("canceller thread completes");
    assert!(
        cancelled,
        "cancel_root must accept the live http/serve root"
    );
    assert!(
        result.is_err(),
        "a cancelled http/serve root must settle as an error, not a value"
    );
    // The WS handler's `ws/recv` parks on a cooperative poll
    // (`ServerWsRecvProbe` / `await_runtime_until`) whose inter-scan wait is a
    // `quarantined_blocking` op: a real (if brief, 5ms) OS-thread sleep that
    // cancellation cannot abort mid-flight — the child task fully reaps only
    // once that in-flight blocking job returns its completion through the
    // inbox. `drive_until_settled` above stops as soon as the ROOT (the
    // accept-loop task) settles, which can race ahead of that last hop for
    // this wait kind specifically (unlike `async/sleep`'s purely-internal
    // timer, which the sibling test above cancels synchronously). Keep
    // driving bounded turns until the count actually reaches 0 — the
    // liveness oracle itself, not a fixed sleep — so this stays a leak gate,
    // not a timing-dependent flake.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut live = interp.runtime_live_task_count();
    while live != 0 && Instant::now() < deadline {
        let _ = interp.drive_turn();
        thread::sleep(Duration::from_millis(2));
        live = interp.runtime_live_task_count();
    }
    assert_eq!(
        live, 0,
        "cancelling the server root must reap the idle WebSocket handler task too — no leak"
    );
}
