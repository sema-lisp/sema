//! Async-context coverage for `http/serve` (WP-SERVE-GUARD, superseded).
//!
//! `http/serve` (`crates/sema-stdlib/src/server.rs`) used to check
//! `in_runtime_quantum() && current_task_id().is_some()` FIRST and fail fast
//! with an explained error whenever called from inside `async/spawn`: its
//! accept loop ran on the calling thread via `rx.blocking_recv()`, and that
//! thread IS the single VM thread the cooperative scheduler drives every task
//! on, so composing it inside `async/spawn` would have frozen the whole
//! process with no error and nothing to debug.
//!
//! SRV-1 (see `docs/deferred.md` §"SRV-1", RESOLVED) removed that guard: the
//! accept loop now parks cooperatively on a re-arming `WaitKind::External`
//! instead of blocking, each connection is its own spawned task, and a
//! server-side WebSocket handler's `(:recv conn)` suspends cooperatively too
//! — so `http/serve` no longer needs to reject `async/spawn` composition; it
//! now genuinely serves from inside one. `http_serve_inside_async_spawn_now_
//! serves` below is the regression gate for that: it replaces the old
//! `http_serve_inside_async_spawn_errors_immediately_no_hang` /
//! `http_serve_guard_does_not_stall_sibling_task` tests, which asserted the
//! now-deleted guard's rejection behavior and would fail (correctly — the
//! guard is gone) against the current code.
//!
//! `crates/sema/tests/http_serve_concurrent_test.rs` is the primary SRV-1
//! acceptance gate (concurrency, WebSocket liveness, cancellation, the
//! top-level regression, and the error-contract pin); this file keeps the
//! narrower top-level-specific regressions (arity validation, a plain
//! top-level serve) plus the async-composition case.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;

// === `http/serve` now genuinely composes inside `async/spawn` ===
//
// Pre-SRV-1, this program would have errored immediately (the guard). Now
// the spawned task's `http/serve` call binds the port and parks its accept
// loop on a cooperative External wait exactly like a top-level call — so
// `async/await`ing it drives the same server, reachable over a real loopback
// connection, while nothing else about the scheduler is disturbed. IN-PROCESS
// (drives the `Interpreter`/`Runtime` host API directly, like
// `http_serve_cancel_test.rs`) so the test can cancel the root itself once
// it has proven the server answers, rather than leaking a live thread/socket
// for the rest of the test binary's process.
#[test]
fn http_serve_inside_async_spawn_now_serves() {
    use sema_vm::runtime::RootOptions;
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::thread;
    use std::time::{Duration, Instant};

    let interp = Interpreter::new();
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        listener.local_addr().expect("local_addr").port()
    };
    let program = format!(
        r#"(async/await (async/spawn (fn ()
             (http/serve (fn (req) (http/text "from-spawn")) {{:port {port}}}))))"#
    );
    let handle = interp
        .submit_str(&program, RootOptions::default())
        .expect("http/serve-inside-async/spawn submits as a root");
    let root_id = handle.id();
    let cmd = interp.command_handle();

    // Prober thread: waits for the port to come up (proving the guard is
    // gone and the spawned accept loop actually bound), makes a real request
    // (proving it actually dispatches), then cancels the root so the driver
    // loop below can return instead of blocking forever (the server, like
    // any `http/serve`, never settles on its own).
    let prober = thread::spawn(move || -> Result<(), String> {
        let addr = format!("127.0.0.1:{port}")
            .to_socket_addrs()
            .map_err(|e| e.to_string())?
            .next()
            .ok_or("no address")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut connected = false;
        while Instant::now() < deadline {
            if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
                connected = true;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        if !connected {
            return Err(
                "http/serve inside async/spawn never bound the port (guard regressed, or a hang)"
                    .to_string(),
            );
        }

        let mut stream =
            TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| e.to_string())?;
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .map_err(|e| e.to_string())?;
        let mut raw = String::new();
        stream.read_to_string(&mut raw).map_err(|e| e.to_string())?;
        if !raw.contains("from-spawn") {
            return Err(format!(
                "server spawned inside async/spawn must actually dispatch requests, got: {raw:?}"
            ));
        }

        if !cmd.cancel_root(root_id) {
            return Err(
                "cancel_root must accept the live http/serve-inside-spawn root".to_string(),
            );
        }
        Ok(())
    });

    let result = interp.drive_until_settled(&handle);
    prober
        .join()
        .expect("prober thread completes")
        .expect("prober thread's checks must all pass");
    assert!(
        result.is_err(),
        "a cancelled http/serve-inside-async/spawn root must settle as an error, not a value"
    );

    // NOT asserting `runtime_live_task_count() == 0` here, unlike
    // `http_serve_cancel_test.rs`'s cancellation gates: a probe
    // (`(async/await (async/spawn (fn () (async/sleep 999999))))`, cancelled
    // the same way) shows `cancel_root` on a root that `async/await`s a
    // spawned child does NOT cascade-cancel that child — the awaited
    // grandchild task is left live even after the awaiting root itself
    // settles as an error. That is a general `async/spawn`+`async/await`
    // cancellation-cascade property, reproducible with no `http/serve`
    // involved at all — orthogonal to SRV-1 (which guarantees a *server's
    // own* descendants — its accept loop and per-connection handler tasks —
    // are reaped when the server itself is cancelled directly, exactly what
    // `http_serve_cancel_test.rs` proves). Flagged in the SRV-1 report as a
    // concern for the next pass rather than fixed here (a sema-vm runtime
    // cascade-cancellation change is out of scope for this piece).
}

// === Sync (top-level) regression: ordinary top-level arg validation still
// === runs (arity error) exactly as before the guard's removal ===
#[test]
fn http_serve_top_level_arity_error_unchanged() {
    let interp = Interpreter::new();
    let err = interp
        .eval_str_compiled("(http/serve)")
        .expect_err("http/serve with no args must still be an arity error at top level");
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("arity") || msg.contains("expects"),
        "top-level arity validation must be unchanged by the guard's removal, got: {msg}"
    );
}

// === Sync (top-level) regression: a real top-level http/serve still binds
// === and answers a request ===
#[test]
#[ignore] // requires network
fn http_serve_top_level_still_serves() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"(http/serve (fn (req) (http/ok {:path (:path req)})) {:port 19938})"#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    sema_llm::http::ensure_crypto_provider();
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("http://127.0.0.1:19938/guard-check")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("failed to GET");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().expect("failed to parse JSON");
    assert_eq!(body["path"], "/guard-check");

    child.kill().ok();
    child.wait().ok();
}
