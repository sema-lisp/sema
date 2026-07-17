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
