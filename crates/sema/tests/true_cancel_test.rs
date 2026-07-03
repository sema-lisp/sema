//! Acceptance gate for Slice B — TRUE cancellation (real socket/process abort).
//!
//! When an `async/timeout` (or `async/cancel`) abandons a task parked on an
//! offloaded `AwaitIo` future, the scheduler now runs the handle's abort hook. For
//! the `spawn`-based subprocess offload that means the in-flight future is aborted
//! and, because the `tokio::process::Command` is `kill_on_drop(true)`, the child
//! process is KILLED — not left running to completion. These tests prove the kill
//! deterministically via a marker file the subprocess only writes if it survives.
//!
//! (The http abort tier uses the same seam — `AbortHandle::abort()` drops the
//! reqwest future — and is covered by the unit tests on `IoHandle` + this
//! subprocess gate. The LLM tier gets its own live-server proof below:
//! `llm_request_is_aborted_on_timeout` stands up a slow local HTTP server, points
//! the ollama provider at it, and observes the client disconnect when the
//! completion is timed out — the request is truly torn down mid-flight, not left
//! to burn money on a worker.)

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use serial_test::serial;
use std::path::PathBuf;
use std::time::Duration;

/// A unique marker path under the system temp dir for one test (removed up front).
fn marker(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "sema_true_cancel_{}_{}.marker",
        name,
        std::process::id()
    ));
    let _ = std::fs::remove_file(&p);
    p
}

/// HEADLINE GATE: the marker is written by a GRANDCHILD (a backgrounded subshell)
/// that `sh` forks, while `sh` itself stays alive (`wait`). On timeout the whole
/// PROCESS GROUP must be killed — so neither `sh` nor the grandchild survives and
/// the marker never appears. This specifically distinguishes a group kill from a
/// kill of only the direct `sh` pid (which would orphan the grandchild, leaving it
/// to `touch` the marker after its sleep).
#[test]
#[serial]
fn subprocess_group_is_killed_on_timeout() {
    let m = marker("killed");
    let interp = Interpreter::new();
    let program = format!(
        r#"(try
             (async/timeout 200
               (async/spawn (fn () (shell "sh" "-c" "(sleep 3; touch {}) & wait"))))
             (catch e :caught))"#,
        m.display()
    );
    let result = interp
        .eval_str_compiled(&program)
        .expect("timeout-abandoned shell evaluated");
    assert_eq!(
        result,
        sema_core::Value::keyword("caught"),
        "the timeout must surface as a caught error"
    );
    // Wait past the grandchild's 3 s sleep. If only `sh` (the direct child) were
    // killed, the orphaned grandchild would `touch` the marker around now.
    std::thread::sleep(Duration::from_millis(4000));
    assert!(
        !m.exists(),
        "the whole process GROUP must be killed on timeout — marker {} should not exist",
        m.display()
    );
    let _ = std::fs::remove_file(&m);
}

/// Cancellation must be TRANSITIVE: a subprocess awaited INDIRECTLY (one
/// `async/await` layer deeper than the timed-out task) must still be killed, and its
/// inner task must not survive as an un-reaped orphan. Before transitive cancel, the
/// timeout cancelled only the outer task and the inner `Blocked(AwaitIo)` shell task
/// ran to completion (marker appeared) AND lingered in the scheduler.
#[test]
#[serial]
fn indirectly_awaited_subprocess_is_killed_on_timeout() {
    let m = marker("indirect");
    let interp = Interpreter::new();
    let program = format!(
        r#"(try
             (async/timeout 200
               (async/spawn (fn ()
                 (async/await
                   (async/spawn (fn () (shell "sh" "-c" "(sleep 3; touch {}) & wait")))))))
             (catch e :caught))"#,
        m.display()
    );
    let result = interp
        .eval_str_compiled(&program)
        .expect("indirect timeout-abandoned shell evaluated");
    assert_eq!(result, sema_core::Value::keyword("caught"));
    // No orphaned inner task left behind (would also be a #7 span-at-teardown hazard
    // for the LLM tier): transitive cancel transitioned it to terminal → reaped.
    assert_eq!(
        sema_vm::scheduler_task_count(),
        0,
        "the indirectly-awaited inner task must be cancelled + reaped, not orphaned"
    );
    std::thread::sleep(Duration::from_millis(4000));
    assert!(
        !m.exists(),
        "an indirectly-awaited subprocess must also be killed — marker {} should not exist",
        m.display()
    );
    let _ = std::fs::remove_file(&m);
}

/// CONTROL: with a timeout LONGER than the subprocess's work, it completes normally
/// and the marker IS written — proving the kill gate above isn't a false positive
/// (e.g. the shell never running at all).
#[test]
#[serial]
fn subprocess_completes_when_timeout_is_longer() {
    let m = marker("completes");
    let interp = Interpreter::new();
    let program = format!(
        r#"(async/timeout 5000
             (async/spawn (fn () (shell "sh" "-c" "sleep 1; touch {}"))))"#,
        m.display()
    );
    interp
        .eval_str_compiled(&program)
        .expect("long-timeout shell evaluated");
    // The shell ran to completion within the timeout, so the marker exists now.
    assert!(
        m.exists(),
        "the subprocess should have completed and written marker {}",
        m.display()
    );
    let _ = std::fs::remove_file(&m);
}

/// A slow local HTTP "LLM" server for the abort proof: accepts ONE connection,
/// records that a request arrived, then holds the connection for `hold_ms`
/// while watching for the client to hang up. If the client disconnects first
/// (`Ok(0)`/reset on read), `client_disconnected` is set and no response is
/// written; if the hold elapses, it writes a valid ollama-shaped reply and sets
/// `finished_response`.
struct SlowLlmServer {
    port: u16,
    saw_request: std::sync::Arc<std::sync::atomic::AtomicBool>,
    client_disconnected: std::sync::Arc<std::sync::atomic::AtomicBool>,
    finished_response: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

fn start_slow_llm_server(hold_ms: u64) -> SlowLlmServer {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind slow llm server");
    let port = listener.local_addr().expect("local_addr").port();
    let saw_request = Arc::new(AtomicBool::new(false));
    let client_disconnected = Arc::new(AtomicBool::new(false));
    let finished_response = Arc::new(AtomicBool::new(false));

    let saw = saw_request.clone();
    let disc = client_disconnected.clone();
    let fin = finished_response.clone();
    std::thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0u8; 8192];
        if sock.read(&mut buf).is_err() {
            return;
        }
        saw.store(true, Ordering::SeqCst);
        // Hold the connection, polling for a client hang-up: a dropped reqwest
        // future closes the socket, which surfaces here as EOF (`Ok(0)`) or a
        // reset — either means the cancel truly tore the request down.
        sock.set_read_timeout(Some(Duration::from_millis(25)))
            .expect("read timeout");
        let deadline = Instant::now() + Duration::from_millis(hold_ms);
        while Instant::now() < deadline {
            match sock.read(&mut buf) {
                Ok(0) => {
                    disc.store(true, Ordering::SeqCst);
                    return;
                }
                Ok(_) => {} // more request bytes; keep holding
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => {
                    disc.store(true, Ordering::SeqCst);
                    return;
                }
            }
        }
        let body = r#"{"message":{"role":"assistant","content":"slow reply"},"prompt_eval_count":1,"eval_count":1}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        if sock.write_all(resp.as_bytes()).is_ok() {
            fin.store(true, Ordering::SeqCst);
        }
    });
    SlowLlmServer {
        port,
        saw_request,
        client_disconnected,
        finished_response,
    }
}

/// Spin until `flag` is set or `ms` elapses; returns whether it was set.
fn wait_for_flag(flag: &std::sync::atomic::AtomicBool, ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        if flag.load(std::sync::atomic::Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    flag.load(std::sync::atomic::Ordering::SeqCst)
}

/// LLM-TIER HEADLINE GATE: timing out an async `llm/complete` ABORTS the
/// in-flight provider request. The ollama provider (keyless) points at a local
/// server that holds the connection for 6 s; the completion is timed out at
/// 300 ms. Proof of true abort: (a) the eval returns in ~timeout, not the
/// server's hold; (b) the SERVER observes the client disconnect — the spawned
/// wire future was dropped and the connection torn down, so no money/connection
/// is burned behind the cancel.
#[test]
#[serial]
fn llm_request_is_aborted_on_timeout() {
    let server = start_slow_llm_server(6000);
    let interp = Interpreter::new();
    sema_llm::builtins::reset_runtime_state();

    // Unique prompt so a stale disk-cache entry can never satisfy the call
    // without touching the network (cache is mem + disk; reset clears only mem).
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let program = format!(
        r#"(llm/configure :ollama {{:host "http://127.0.0.1:{port}" :default-model "test-model"}})
           (try
             (async/timeout 300
               (async/spawn (fn () (llm/complete "abort-proof-{nonce}"))))
             (catch e :caught))"#,
        port = server.port,
    );

    let t0 = std::time::Instant::now();
    let result = interp
        .eval_str_compiled(&program)
        .expect("timed-out llm/complete evaluated");
    let elapsed = t0.elapsed();

    eprintln!("[abort-proof] eval elapsed = {elapsed:?}");
    assert_eq!(
        result,
        sema_core::Value::keyword("caught"),
        "the timeout must surface as a caught error"
    );
    assert!(
        elapsed < Duration::from_millis(2500),
        "eval must return around the 300 ms timeout, not the server's 6 s hold; took {elapsed:?}"
    );
    assert!(
        server.saw_request.load(std::sync::atomic::Ordering::SeqCst),
        "the provider request must actually have reached the server"
    );
    let t1 = std::time::Instant::now();
    let disconnected = wait_for_flag(&server.client_disconnected, 3000);
    eprintln!(
        "[abort-proof] disconnect observed {:?} after eval returned",
        t1.elapsed()
    );
    assert!(
        disconnected,
        "the server must observe the client disconnect — the in-flight request \
         was aborted, not left running to completion"
    );
    assert!(
        !server
            .finished_response
            .load(std::sync::atomic::Ordering::SeqCst),
        "the server must not have finished writing a response"
    );
}

/// DEFAULT-IMPL TIER (documented limit): a provider WITHOUT a native async path
/// (`complete_future` → `None`; here the FakeProvider test double) runs its sync
/// `complete()` on the pool's blocking tier inside the spawned wire future.
/// Aborting that future discards the RESULT, but the blocking call cannot be
/// interrupted and runs to completion on the worker — cancellation stays
/// best-effort for sync-only providers. This test pins that tier: the timeout
/// returns promptly, nothing panics, and the fake's `complete()` was invoked
/// (and left to finish) despite the cancel.
#[test]
#[serial]
fn sync_only_provider_cancel_is_best_effort() {
    let interp = Interpreter::new();
    sema_llm::builtins::reset_runtime_state();
    let fake = sema_llm::fake::FakeProvider::builder("fake")
        .chat_delay(800)
        .reply("too late")
        .build();
    let recorder = fake.recorder();
    sema_llm::builtins::register_test_provider(Box::new(fake));

    let program = r#"
        (try
          (async/timeout 150
            (async/spawn (fn () (llm/complete "best-effort-cancel"))))
          (catch e :caught))"#;
    let t0 = std::time::Instant::now();
    let result = interp
        .eval_str_compiled(program)
        .expect("timed-out fake llm/complete evaluated");
    let elapsed = t0.elapsed();

    assert_eq!(result, sema_core::Value::keyword("caught"));
    assert!(
        elapsed < Duration::from_millis(600),
        "the task must be released at the 150 ms timeout, not after the fake's \
         800 ms blocking delay; took {elapsed:?}"
    );
    // Let the detached blocking call run out — it completes on the worker; its
    // result is discarded. No panic, no wedged pool.
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(
        recorder.call_count(),
        1,
        "the fake's complete() was dispatched and ran despite the cancel \
         (best-effort tier: result discarded, work not interrupted)"
    );
}

/// A normally-completing concurrent subprocess must return its real output and must
/// NOT be aborted (the abort hook fires only on cancel/timeout/interrupt).
#[test]
#[serial]
fn normal_completion_returns_output_and_is_not_aborted() {
    let interp = Interpreter::new();
    let program = r#"
        (let ((r (async/await (async/spawn (fn () (shell "sh" "-c" "echo hello"))))))
          (string/trim (:stdout r)))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("normal concurrent shell evaluated");
    assert_eq!(
        result,
        sema_core::Value::string("hello"),
        "a normally-completing shell must return its stdout, never be aborted"
    );
}
