//! P0 acceptance gate: the runtime's External-wait ASYNC tier (`ProcessIoExecutor`
//! in sema-io, driving `ExecutorDispatch::Async` on the shared tokio reactor).
//!
//! http is the reference External op converted onto `external_io_async`
//! (`crates/sema-stdlib/src/http.rs` → `runtime_offload::external_io_async`), so
//! these tests exercise the async tier end-to-end through `http/get`:
//!
//! 1. `http_16_concurrent_overlap` — sixteen 300 ms requests via
//!    `async/spawn`+`async/all` complete in ~1× wall-time, decisively below the
//!    16× serial floor AND below the ceiling the old blocking tier imposed (one
//!    pool worker per in-flight op, clamped to [2,8] → ≥2 serial batches at N=16).
//! 2. `http_get_cancel_aborts_and_disconnects` — an `async/cancel` of a parked
//!    `http/get` returns promptly and the SERVER observes the client disconnect
//!    (the in-flight reqwest future was dropped, socket torn down).
//! 3. `fake_agent_concurrency_overlaps_via_runtime` — the sema-llm runtime
//!    completion path (`do_complete_runtime_suspend`, `interruptible_async`) runs
//!    on the real reactor: concurrent FakeProvider agents overlap in flight.
//!
//! A `#[ignore]`d live smoke (`live_agent_run_via_runtime_smoke`) drives the same
//! sema-llm async tier against a real provider (cheap model) when keys are set.

#![cfg(not(target_arch = "wasm32"))]

use std::io::Read;
use std::net::TcpListener as StdTcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sema_core::Value;
use sema_eval::Interpreter;
use sema_llm::builtins::{
    io_peak_inflight, register_test_provider, reset_io_inflight, reset_runtime_state,
};
use sema_llm::fake::FakeProvider;
use serial_test::serial;

/// Per-request delay of the local overlap server, in milliseconds.
const DELAY_MS: u64 = 300;

/// Spin up a local delay HTTP server (raw `tokio` TCP on its own multi-thread
/// runtime, random port). The handler sleeps [`DELAY_MS`] then echoes the `i`
/// query param as `echo:<i>`. Returns the bound port; the runtime is leaked so
/// the accept loop lives for the whole test.
fn start_delay_server() -> u16 {
    let std_listener = StdTcpListener::bind("127.0.0.1:0").expect("bind delay server");
    std_listener.set_nonblocking(true).expect("set_nonblocking");
    let port = std_listener.local_addr().expect("local_addr").port();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("delay server runtime");

    rt.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("from_std listener");
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => continue,
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 1024];
                let n = match socket.read(&mut buf).await {
                    Ok(n) => n,
                    Err(_) => return,
                };
                let req = String::from_utf8_lossy(&buf[..n]);
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

    Box::leak(Box::new(rt));
    std::thread::sleep(Duration::from_millis(50));
    port
}

/// Sixteen 300 ms requests overlap on the async tier: wall-clock ~1× (well under
/// the 16× serial floor of ~4800 ms and under the old blocking-tier ceiling of
/// ≥2 serial batches). Bodies must be correct and in input order.
#[test]
#[serial]
fn http_16_concurrent_overlap() {
    let port = start_delay_server();
    let interp = Interpreter::new();
    let program = format!(
        r#"
        (async/all
          (map (fn (i)
                 (async/spawn
                   (fn () (:body (http/get (string-append "http://127.0.0.1:{port}/d?i=" (number->string i)))))))
               (list 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)))
        "#
    );

    let t0 = Instant::now();
    let result = interp
        .eval_str_compiled(&program)
        .expect("16 concurrent http program evaluated");
    let elapsed_ms = t0.elapsed().as_millis();

    let expected = Value::list(
        (0..16)
            .map(|i| Value::string(&format!("echo:{i}")))
            .collect(),
    );
    assert_eq!(result, expected, "expected 16 echoed bodies in input order");

    eprintln!("http_16_concurrent_overlap: wall-clock {elapsed_ms} ms (serial floor ~4800 ms)");
    // Fully overlapped is ~300-900 ms; the old [2,8]-worker blocking ceiling would
    // force ≥2 batches (~600 ms floor at 8 workers, ~2400 ms at 2). 1500 ms proves
    // we are well past the worker ceiling while leaving slack for CI jitter.
    assert!(
        elapsed_ms < 1500,
        "expected overlapped wall-clock < 1500 ms at N=16 (serial floor ~4800 ms), got {elapsed_ms} ms"
    );
}

/// A slow HTTP server that accepts ONE connection, records that a request
/// arrived, then holds the connection for `hold_ms` while watching for the client
/// to hang up. A dropped reqwest future closes the socket → observed here as
/// EOF/reset → `client_disconnected` set.
struct SlowServer {
    port: u16,
    saw_request: Arc<AtomicBool>,
    client_disconnected: Arc<AtomicBool>,
}

fn start_slow_server(hold_ms: u64) -> SlowServer {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind slow server");
    let port = listener.local_addr().expect("local_addr").port();
    let saw_request = Arc::new(AtomicBool::new(false));
    let client_disconnected = Arc::new(AtomicBool::new(false));

    let saw = saw_request.clone();
    let disc = client_disconnected.clone();
    std::thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0u8; 8192];
        if sock.read(&mut buf).is_err() {
            return;
        }
        saw.store(true, Ordering::SeqCst);
        sock.set_read_timeout(Some(Duration::from_millis(25)))
            .expect("read timeout");
        let deadline = Instant::now() + Duration::from_millis(hold_ms);
        while Instant::now() < deadline {
            match sock.read(&mut buf) {
                Ok(0) => {
                    disc.store(true, Ordering::SeqCst);
                    return;
                }
                Ok(_) => {}
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => {
                    disc.store(true, Ordering::SeqCst);
                    return;
                }
            }
        }
    });
    SlowServer {
        port,
        saw_request,
        client_disconnected,
    }
}

/// Spin until `flag` is set or `ms` elapses; returns whether it was set.
fn wait_for_flag(flag: &AtomicBool, ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        if flag.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    flag.load(Ordering::SeqCst)
}

/// `async/cancel` of a parked `http/get` aborts the in-flight request: the eval
/// returns around the 200 ms cancellation (not the server's 6 s hold), and the
/// SERVER observes the client disconnect — the async-tier future was dropped and
/// the socket torn down.
#[test]
#[serial]
fn http_get_cancel_aborts_and_disconnects() {
    let server = start_slow_server(6000);
    let interp = Interpreter::new();
    let program = format!(
        r#"(define p (async/spawn (fn () (http/get "http://127.0.0.1:{port}/slow"))))
           (async/spawn (fn () (async/sleep 200) (async/cancel p)))
           (try (async/await p) (catch e :caught))"#,
        port = server.port,
    );

    let t0 = Instant::now();
    let result = interp
        .eval_str_compiled(&program)
        .expect("cancelled http/get evaluated");
    let elapsed = t0.elapsed();

    eprintln!("http_get_cancel: eval elapsed = {elapsed:?}");
    assert_eq!(
        result,
        Value::keyword("caught"),
        "explicit cancellation must surface as a caught error"
    );
    assert!(
        elapsed < Duration::from_millis(2000),
        "eval must return around the 200 ms cancellation, not the 6 s hold; took {elapsed:?}"
    );
    assert!(
        server.saw_request.load(Ordering::SeqCst),
        "the request must actually have reached the server"
    );
    assert!(
        wait_for_flag(&server.client_disconnected, 3000),
        "the server must observe the client disconnect — the in-flight request was \
         aborted on the async tier, not left running to completion"
    );
}

/// The sema-llm runtime completion path (`do_complete_runtime_suspend`, an
/// `interruptible_async` op) runs on the real reactor now: three concurrent
/// FakeProvider `agent/run`s driven through the unified runtime overlap in flight
/// (peak in-flight ≥ 2) and finish well under the serial floor.
#[test]
#[serial]
fn fake_agent_concurrency_overlaps_via_runtime() {
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(120)
        .tool_loop(2, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 6})
        (let ((t0 (sys/elapsed)))
          (async/all
            (map (fn (i) (async/spawn (fn () (agent/run bot "go"))))
                 (list 1 2 3)))
          (floor (/ (- (sys/elapsed) t0) 1000000)))
    "#;
    let wall = interp
        .eval_str_via_runtime(program)
        .expect("3 concurrent agents evaluated through the runtime");
    let wall_ms = wall.as_int().expect("wall ms");

    assert!(
        io_peak_inflight() >= 2,
        "expected peak offloaded futures in flight >= 2 (agents overlapping), got {}",
        io_peak_inflight()
    );
    assert!(
        wall_ms < 700,
        "expected overlapped wall < 700 ms (serial floor ~1080 ms), got {wall_ms} ms"
    );
}

/// Live smoke: drive the sema-llm runtime async tier against a REAL provider (a
/// cheap model) through the unified runtime, so `do_complete_runtime_suspend`
/// runs a real `reqwest` future on the reactor. `#[ignore]` so CI never hits the
/// network; run manually with keys set:
///   `cargo test -p sema-lang --test runtime_external_async_test -- --ignored --nocapture`
#[test]
#[ignore = "hits a real LLM provider; run manually with ANTHROPIC_API_KEY set"]
#[serial]
fn live_agent_run_via_runtime_smoke() {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("live_agent_run_via_runtime_smoke: ANTHROPIC_API_KEY unset — skipping");
        return;
    }
    let key = std::env::var("ANTHROPIC_API_KEY").expect("checked above");
    let interp = Interpreter::new();
    reset_runtime_state();
    let program = format!(
        r#"
        (llm/configure :anthropic {{:api-key "{key}" :default-model "claude-haiku-4-5"}})
        (defagent bot {{:model "claude-haiku-4-5" :system "Reply with exactly one word."}})
        (agent/run bot "Reply with the single word: pong")
    "#
    );
    let program = program.as_str();
    let t0 = Instant::now();
    let result = interp
        .eval_str_via_runtime(program)
        .expect("live agent/run through the runtime async tier");
    eprintln!(
        "live_agent_run_via_runtime_smoke: {result:?} in {:?}",
        t0.elapsed()
    );
    // Any non-error string answer proves the async tier drove a real reqwest
    // future to completion on the reactor.
    assert!(
        result.is_string(),
        "expected a string answer from the live provider, got {result:?}"
    );
}
