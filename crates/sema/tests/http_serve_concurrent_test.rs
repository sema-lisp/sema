//! Acceptance gate for **SRV-1** — async `http/serve` + concurrent, non-blocking
//! connection handling.
//!
//! SRV-1 is landed (see `docs/deferred.md` §"SRV-1", RESOLVED): the accept
//! loop (`crates/sema-stdlib/src/server.rs`'s `http_serve_runtime_impl`) parks
//! on a re-arming `WaitKind::External` fed by the tokio request channel
//! instead of blocking the VM thread, each connection's handler runs as its
//! own spawned task, and a server-side WebSocket handler's `(:recv conn)`
//! (`handle_ws_response_runtime`) suspends cooperatively too — so a slow,
//! async-parked, or WebSocket-idling handler no longer blocks its siblings,
//! and the old fail-fast guard against `http/serve` inside `async/spawn` is
//! gone. Every test below is a standing regression gate against that
//! contract, not a failing-test-first artifact anymore.
//!
//! ## Why `#[ignore]` is now unused here
//!
//! None of these tests are `#[ignore]`d: they bind a loopback port and spawn
//! the `sema` binary as a subprocess (like
//! `server_async_test::http_serve_top_level_still_serves`), but that alone
//! isn't disqualifying — sibling subprocess/network tests in this same crate
//! (e.g. `http_serve_cancel_test.rs`'s in-process variant, `server_test.rs`)
//! already run un-ignored in the default `cargo test` gate. `regression_top_
//! level_serve_still_answers` was previously ignored for "needs a loopback
//! port + subprocess" alone, which is inconsistent with its un-ignored
//! siblings in this very file that use the identical pattern — un-ignored
//! here to match.
//!
//! ## Bounded guards
//!
//! A regression must manifest as a **bounded failure, never a suite hang**: every
//! request uses a wall-clock client timeout, every subprocess is killed on the
//! way out, and every worker thread is joined with a bounded `recv_timeout`. A
//! serial-dispatch regression therefore surfaces as a timeout→assertion failure.

#![cfg(not(target_arch = "wasm32"))]

use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Bind an ephemeral loopback port, then release it so the sema subprocess can
/// claim it. A tiny reuse race is possible but negligible for a local test.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local_addr").port()
}

/// Spawn `sema -e <program>`, returning the child. Caller must kill it.
fn spawn_serve(program: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(program)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn sema subprocess")
}

/// Poll the port until a TCP connection succeeds (server bound) or `deadline`
/// elapses. Returns whether the server came up in time.
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
        thread::sleep(Duration::from_millis(50));
    }
    false
}

/// A minimal blocking HTTP/1.1 GET with a hard socket timeout. Returns the raw
/// response body (everything after the header terminator). Kept dependency-free
/// and bounded so a stalled server fails the test rather than hanging it.
fn http_get_body(port: u16, path: &str, timeout: Duration) -> Result<String, String> {
    let addr = format!("127.0.0.1:{port}")
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("no address")?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    use std::io::Write;
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).map_err(|e| e.to_string())?;
    let body = raw
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok(body)
}

/// Like `http_get_body`, but also returns the HTTP status code — needed for
/// `uncaught_handler_error_produces_the_bounded_500_fallback`, which pins
/// both.
fn http_get_status_and_body(
    port: u16,
    path: &str,
    timeout: Duration,
) -> Result<(u16, String), String> {
    let addr = format!("127.0.0.1:{port}")
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("no address")?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    use std::io::Write;
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).map_err(|e| e.to_string())?;
    let (status_line, rest) = raw.split_once("\r\n").ok_or("no status line")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or("no status code")?
        .parse()
        .map_err(|_| "status code not numeric".to_string())?;
    let body = rest
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

/// Run `body` on a worker thread and require it to finish within `budget`,
/// returning its result. A hung server therefore fails the test with a clear
/// message instead of blocking the harness forever.
fn bounded<T: Send + 'static>(budget: Duration, body: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let _ = tx.send(body());
    });
    let out = rx
        .recv_timeout(budget)
        .expect("operation exceeded its wall-clock budget (SRV-1 serial-dispatch regression?)");
    let _ = handle.join();
    out
}

/// **Scenario 1 — concurrency: a slow handler must not stall a fast one.**
///
/// `/slow` sleeps 300 ms (via `async/sleep`) then replies; `/fast` replies
/// immediately. We start `/slow` first, wait long enough for its handler to be
/// dispatched and parked, then time a `/fast` request. Under concurrent dispatch
/// `/fast` returns in well under the slow handler's remaining sleep; under the
/// shipped serial loop it can only be handled after `/slow` completes.
#[test]
fn slow_handler_does_not_block_fast_handler() {
    let port = free_port();
    let program = format!(
        r#"(http/serve
             (fn (req)
               (if (= (:path req) "/slow")
                   (begin (async/sleep 300) (http/text "slow"))
                   (http/text "fast")))
             {{:port {port}}})"#
    );
    let mut child = spawn_serve(&program);
    assert!(
        wait_until_listening(port, Duration::from_secs(5)),
        "server never bound"
    );

    // Kick off /slow and give its handler ~120 ms to be picked up and start
    // sleeping, so /fast is demonstrably issued while /slow is in flight.
    let slow = thread::spawn(move || http_get_body(port, "/slow", Duration::from_secs(5)));
    thread::sleep(Duration::from_millis(120));

    let t0 = Instant::now();
    let fast = bounded(Duration::from_secs(3), move || {
        http_get_body(port, "/fast", Duration::from_secs(3))
    });
    let fast_ms = t0.elapsed().as_millis();

    child.kill().ok();
    child.wait().ok();
    let _ = slow.join();

    assert_eq!(fast.as_deref(), Ok("fast"), "fast handler body wrong");
    assert!(
        fast_ms < 180,
        "fast handler overlapped the slow one? took {fast_ms} ms (serial dispatch would be ~180 ms+)"
    );
}

/// **Scenario 2 — the documented head-of-line pathology.** A WebSocket handler
/// idling in `ws/recv` on one connection must not prevent a second connection's
/// plain HTTP request from being served. The WS client connects and stays
/// silent; a plain `/ping` GET must still complete within a bounded time.
///
/// This is the strongest failing-first demonstrator: against the shipped serial
/// loop the WS handler's `ws/recv` (`blocking_recv`) pins the single evaluator
/// thread, so `/ping` never returns — the bounded guard turns that permanent
/// stall into a test failure.
#[test]
fn idle_websocket_does_not_block_plain_request() {
    let port = free_port();
    let program = format!(
        r#"(http/serve
             (fn (req)
               (if (= (:path req) "/ws")
                   (http/websocket (fn (conn)
                     (let loop ()
                       (let ((msg ((:recv conn))))
                         (if (null? msg) nil (begin ((:send conn) msg) (loop)))))))
                   (http/text "pong")))
             {{:port {port}}})"#
    );
    let mut child = spawn_serve(&program);
    assert!(
        wait_until_listening(port, Duration::from_secs(5)),
        "server never bound"
    );

    // Open a WS connection and hold it idle (never send a frame).
    let ws_url = format!("ws://127.0.0.1:{port}/ws");
    let ws = tungstenite::connect(&ws_url);
    assert!(ws.is_ok(), "WS upgrade failed: {:?}", ws.err());
    let (_socket, _resp) = ws.unwrap();

    // A plain request must still be served promptly despite the idle WS handler.
    let ping = bounded(Duration::from_secs(3), move || {
        http_get_body(port, "/ping", Duration::from_secs(3))
    });

    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        ping.as_deref(),
        Ok("pong"),
        "plain request blocked behind an idle WebSocket handler (head-of-line)"
    );
}

/// **Scenario 3 — a handler that parks on real async still returns correctly.**
/// The handler awaits a spawned task (which itself sleeps) and echoes its result;
/// the response must come back intact within a bounded time.
#[test]
fn handler_parking_on_async_returns_response() {
    let port = free_port();
    let program = format!(
        r#"(http/serve
             (fn (req)
               (http/text (async/await (async/spawn (fn () (begin (async/sleep 100) "awaited"))))))
             {{:port {port}}})"#
    );
    let mut child = spawn_serve(&program);
    assert!(
        wait_until_listening(port, Duration::from_secs(5)),
        "server never bound"
    );

    let body = bounded(Duration::from_secs(3), move || {
        http_get_body(port, "/", Duration::from_secs(3))
    });

    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        body.as_deref(),
        Ok("awaited"),
        "handler awaiting a spawned task did not return its result"
    );
}

/// **Scenario 4 — regression guard.** Ordinary top-level `http/serve` (single
/// handler, echoing the request path) must keep answering. This already passes
/// against the shipped serial design; it is ignored only because it needs a
/// loopback port + subprocess. The SRV-1 landing must keep it green.
#[test]
fn regression_top_level_serve_still_answers() {
    let port = free_port();
    let program = format!(r#"(http/serve (fn (req) (http/text (:path req))) {{:port {port}}})"#);
    let mut child = spawn_serve(&program);
    assert!(
        wait_until_listening(port, Duration::from_secs(5)),
        "server never bound"
    );

    let body = bounded(Duration::from_secs(3), move || {
        http_get_body(port, "/echo-me", Duration::from_secs(3))
    });

    child.kill().ok();
    child.wait().ok();

    assert_eq!(body.as_deref(), Ok("/echo-me"), "top-level serve regressed");
}

/// **Error-contract pin.** A handler that raises (never returns, so
/// `http/serve`'s responder native is never called — see
/// `crates/sema-stdlib/src/server.rs`'s `make_responder_native` doc comment)
/// must produce a bounded 500 with a fixed, pinned body. This is the CHOSEN
/// contract from the SRV-1 piece-b/c concerns list: the pre-SRV-1 serial
/// loop's `{"error": "..."}` JSON body is undocumented
/// (`website/docs/stdlib/web-server.md` only documents explicit
/// `http/error`/`http/not-found`/etc. constructors, never an implicit
/// uncaught-exception shape) and `server_test.rs`'s `test_http_serve_handler_
/// error` only ever asserted the status code — so there is no compatibility
/// obligation to restore the old JSON shape, and this test pins whatever the
/// concurrent accept loop actually produces instead of leaving it untested.
#[test]
fn uncaught_handler_error_produces_the_bounded_500_fallback() {
    let port = free_port();
    let program = format!(r#"(http/serve (fn (req) (error "boom")) {{:port {port}}})"#);
    let mut child = spawn_serve(&program);
    assert!(
        wait_until_listening(port, Duration::from_secs(5)),
        "server never bound"
    );

    let result = bounded(Duration::from_secs(3), move || {
        http_get_status_and_body(port, "/anything", Duration::from_secs(3))
    });

    child.kill().ok();
    child.wait().ok();

    let (status, body) = result.expect("request must complete (bounded 500, not a hang)");
    assert_eq!(status, 500, "uncaught handler error must produce a 500");
    assert_eq!(
        body, "Handler did not respond",
        "uncaught handler error's body is the pinned bounded-fallback text, not the legacy JSON shape"
    );
}
