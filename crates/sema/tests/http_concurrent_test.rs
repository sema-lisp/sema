//! Acceptance gate for concurrent HTTP (`http/*` overlapping under `async/spawn`).
//!
//! `http/get` and friends funnel through `http_request` in
//! `crates/sema-stdlib/src/http.rs`. At top level they `block_on` the per-thread
//! runtime (synchronous, unchanged). Inside an `async/spawn`'d task they offload
//! the round-trip onto a process-wide multi-thread runtime and yield `AwaitIo`,
//! so several requests overlap on the single VM thread.
//!
//! These tests stand up a **local delay HTTP server** (raw `tokio` TCP, on its
//! own multi-thread runtime, on a random port) so they are deterministic and
//! network-free. The handler sleeps 300 ms then echoes the `i` query param.
//!
//! - Overlap: five 300 ms requests via `async/all`+`async/spawn` complete in
//!   ~300-900 ms (overlapped), decisively below the ~1500 ms serial floor, and
//!   each body is correct & in input order.
//! - Error path: a concurrent request to a closed port fails that task cleanly
//!   without hanging the scheduler.
//! - Sync path unchanged: a plain top-level `http/get` still works (status/body).
//! - Live benchmark (`#[ignore]`): hits a real delay endpoint to eyeball overlap.

#![cfg(not(target_arch = "wasm32"))]

use std::io::Write;
use std::net::TcpListener as StdTcpListener;
use std::time::{Duration, Instant};

use sema_core::Value;
use sema_eval::Interpreter;
use serial_test::serial;

/// Per-request delay of the local test server, in milliseconds.
const DELAY_MS: u64 = 300;

/// Spin up a local delay HTTP server on its own multi-thread tokio runtime.
///
/// The handler parses the request line, sleeps [`DELAY_MS`], then replies with a
/// body that echoes the `i` query parameter (`echo:<i>`), so the test can assert
/// per-request correctness and ordering. Returns the bound port. The runtime is
/// intentionally leaked (`Box::leak`) so it lives for the whole test process.
fn start_delay_server() -> u16 {
    start_delay_server_with_gauge().0
}

/// Like [`start_delay_server`], additionally returning a gauge of the maximum
/// number of requests the server ever had in flight simultaneously. Server-side
/// overlap is a load-independent oracle: a wall-clock bound can trip on a busy
/// test machine, but "the server held >= 2 requests inside their delay windows
/// at once" is exactly the overlap property and nothing else.
fn start_delay_server_with_gauge() -> (u16, std::sync::Arc<MaxInFlight>) {
    // Bind synchronously first so we can hand the caller a ready port with no
    // race against the background accept loop.
    let std_listener = StdTcpListener::bind("127.0.0.1:0").expect("bind delay server");
    std_listener.set_nonblocking(true).expect("set_nonblocking");
    let port = std_listener.local_addr().expect("local_addr").port();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("delay server runtime");

    let gauge = std::sync::Arc::new(MaxInFlight::default());
    let gauge_for_server = gauge.clone();
    rt.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("from_std listener");
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => continue,
            };
            let gauge = gauge_for_server.clone();
            tokio::spawn(async move {
                let _in_flight = gauge.enter();
                use tokio::io::{AsyncReadExt, AsyncWriteExt};

                // Read the request (enough to get the request line). We only
                // need the path/query; a single read covers a small GET.
                let mut buf = [0u8; 1024];
                let n = match socket.read(&mut buf).await {
                    Ok(n) => n,
                    Err(_) => return,
                };
                let req = String::from_utf8_lossy(&buf[..n]);
                // Request line: "GET /d?i=3 HTTP/1.1"
                let path = req
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                let i = path
                    .split("i=")
                    .nth(1)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();

                tokio::time::sleep(Duration::from_millis(DELAY_MS)).await;

                let body = format!("echo:{i}");
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    });

    // Leak the runtime so the accept loop keeps running for the test's lifetime.
    Box::leak(Box::new(rt));
    let ret_gauge = gauge;

    // Best-effort readiness wait: the runtime starts accepting promptly, but a
    // tiny sleep avoids a connection-refused race on the very first request.
    std::thread::sleep(Duration::from_millis(50));
    (port, ret_gauge)
}

/// Tracks the high-water mark of concurrently in-flight requests.
#[derive(Default)]
struct MaxInFlight {
    current: std::sync::atomic::AtomicUsize,
    max: std::sync::atomic::AtomicUsize,
}

impl MaxInFlight {
    fn enter(self: &std::sync::Arc<Self>) -> InFlightGuard {
        use std::sync::atomic::Ordering;
        let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(now, Ordering::SeqCst);
        InFlightGuard(self.clone())
    }

    fn max_seen(&self) -> usize {
        self.max.load(std::sync::atomic::Ordering::SeqCst)
    }
}

struct InFlightGuard(std::sync::Arc<MaxInFlight>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0
            .current
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Find a port that is currently closed (bound then released) so a request to it
/// fails fast with connection-refused.
fn closed_port() -> u16 {
    let l = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let port = l.local_addr().expect("addr").port();
    drop(l);
    port
}

/// Five 300 ms requests run as five tasks via `async/all`+`async/spawn`+`map`.
/// Overlap means ~300-900 ms, not the ~1500 ms serial floor. Bodies must be
/// correct and in input order.
#[test]
#[serial]
fn http_concurrent_overlap() {
    let (port, gauge) = start_delay_server_with_gauge();
    let interp = Interpreter::new();
    let program = format!(
        r#"
        (async/all
          (map (fn (i)
                 (async/spawn
                   (fn () (:body (http/get (string-append "http://127.0.0.1:{port}/d?i=" (number->string i)))))))
               (list 0 1 2 3 4)))
        "#
    );

    let t0 = Instant::now();
    let result = interp
        .eval_str_compiled(&program)
        .expect("concurrent http program evaluated");
    let elapsed_ms = t0.elapsed().as_millis();

    // Correctness: five bodies, echoing 0..=4 in spawn (input) order.
    let expected = Value::list(
        (0..5)
            .map(|i| Value::string(&format!("echo:{i}")))
            .collect(),
    );
    assert_eq!(
        result, expected,
        "expected five echoed bodies in input order"
    );

    // Overlap: the server-side in-flight high-water mark is the oracle — it is
    // immune to test-machine load, unlike a wall-clock bound (which flaked
    // repeatedly under full-parallel nextest runs). Serial execution can never
    // exceed 1 in flight; genuine overlap shows >= 2 (typically 5). Wall-clock
    // is reported for eyeballing only.
    let max_in_flight = gauge.max_seen();
    eprintln!(
        "http_concurrent_overlap: wall-clock {elapsed_ms} ms (serial floor ~1500 ms), \
         max in-flight {max_in_flight}"
    );
    assert!(
        max_in_flight >= 2,
        "expected overlapped requests (server-side max in-flight >= 2), got {max_in_flight} \
         (wall-clock {elapsed_ms} ms)"
    );
}

/// A concurrent request to a closed port must fail that task cleanly and surface
/// the error through `async/all` — without hanging the scheduler.
#[test]
#[serial]
fn http_concurrent_error_path() {
    let port = closed_port();
    let interp = Interpreter::new();
    let program = format!(
        r#"
        (async/all
          (list
            (async/spawn (fn () (http/get "http://127.0.0.1:{port}/")))))
        "#
    );

    let t0 = Instant::now();
    let result = interp.eval_str_compiled(&program);
    let elapsed_ms = t0.elapsed().as_millis();

    // The connection-refused must propagate as an error, not hang.
    assert!(
        result.is_err(),
        "expected the closed-port request to fail the task, got {result:?}"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("http GET") && msg.contains(&format!("127.0.0.1:{port}")),
        "expected an http error mentioning the url, got: {msg}"
    );
    assert!(
        elapsed_ms < 5000,
        "error path should fail fast, not hang; took {elapsed_ms} ms"
    );
}

/// The synchronous (top-level, non-async) path must be untouched: a plain
/// `http/get` against the local server returns the correct status and body.
#[test]
#[serial]
fn http_sync_path_unchanged() {
    let port = start_delay_server();
    let interp = Interpreter::new();
    let program = format!(
        r#"
        (let ((resp (http/get "http://127.0.0.1:{port}/d?i=7")))
          (list (:status resp) (:body resp)))
        "#
    );

    let result = interp
        .eval_str_compiled(&program)
        .expect("sync http program evaluated");
    let expected = Value::list(vec![Value::int(200), Value::string("echo:7")]);
    assert_eq!(
        result, expected,
        "sync path must return status 200 and the echoed body"
    );
}

/// Live benchmark against a real delay endpoint, comparing serial vs concurrent
/// wall-clock. `#[ignore]` so CI never depends on the network.
///
/// Run with: `cargo test -p sema-lang --test http_concurrent_test -- --ignored --nocapture`
#[test]
#[ignore = "hits the network (httpbin.org); run manually for live verification"]
fn http_concurrent_live_benchmark() {
    let interp = Interpreter::new();

    // Serial: five sequential 1 s requests.
    let serial_program = r#"
        (map (fn (_) (:status (http/get "https://httpbin.org/delay/1")))
             (list 0 1 2 3 4))
    "#;
    let t0 = Instant::now();
    let _ = interp.eval_str_compiled(serial_program);
    let serial_ms = t0.elapsed().as_millis();

    // Concurrent: five overlapping 1 s requests.
    let concurrent_program = r#"
        (async/all
          (map (fn (_) (async/spawn (fn () (:status (http/get "https://httpbin.org/delay/1")))))
               (list 0 1 2 3 4)))
    "#;
    let t1 = Instant::now();
    let _ = interp.eval_str_compiled(concurrent_program);
    let concurrent_ms = t1.elapsed().as_millis();

    // Avoid an unused-import warning when this gate body changes; print results.
    let _ = std::io::stderr().flush();
    eprintln!("LIVE http benchmark: serial = {serial_ms} ms, concurrent = {concurrent_ms} ms");
    assert!(
        concurrent_ms < serial_ms,
        "concurrent ({concurrent_ms} ms) should beat serial ({serial_ms} ms)"
    );
}
