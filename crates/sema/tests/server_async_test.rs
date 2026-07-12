//! Async-context guard coverage for `http/serve` (WP-SERVE-GUARD).
//!
//! `http/serve` (`crates/sema-stdlib/src/server.rs`) runs a blocking accept
//! loop on the calling thread for the life of the server
//! (`rx.blocking_recv()` in its dispatch loop) — correct at top level, where
//! that thread has nothing else to do, but catastrophic inside `async/spawn`:
//! that thread IS the VM thread the cooperative scheduler drives every task
//! on, so the loop would never return control to it and every sibling task
//! (indeed the whole process) freezes forever with no error and nothing to
//! debug. Rather than the full non-blocking rearchitecture (a yield-aware
//! dispatch loop with a handler task per connection — real design work,
//! deliberately deferred, see `docs/deferred.md`), `http/serve` now checks
//! `in_async_context()` FIRST, before doing anything else (parsing options,
//! binding a port, spawning the axum future), and fails fast with an
//! explained error instead of hanging silently.
//!
//! A dedicated Rust-level unit test in `server.rs` itself
//! (`http_serve_errors_immediately_in_async_context`) exercises the raw
//! `SemaError` (including `.hint()`) directly, bypassing the scheduler: a
//! task's rejection is flattened to a plain string when it crosses the
//! promise boundary (`format!("{e}")` in `sema-vm/src/scheduler.rs`), so
//! `.hint()` does not survive an `async/await` round-trip — only the core
//! message does. The end-to-end tests here go through the real scheduler via
//! `async/spawn`/`async/await` and assert on that surviving core message,
//! plus (with a hard timeout guard) that the call returns at all rather than
//! hanging.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;

// === The guard fires immediately — no hang — when called inside async/spawn ===
//
// Pre-guard, this program would never return: the spawned task's
// `http/serve` call would bind a port and sit in its blocking accept loop
// forever, starving the scheduler so `async/await` never gets a chance to
// even notice the task is stuck (the whole process would just hang). Run the
// eval on a background thread and bound the wait with `recv_timeout` so a
// regression here fails the test instead of hanging the suite.
#[test]
fn http_serve_inside_async_spawn_errors_immediately_no_hang() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let interp = Interpreter::new();
        let program = r#"
            (async/await (async/spawn (fn ()
              (http/serve (fn (req) (http/ok "hi")) {:port 19939}))))
        "#;
        let result = interp.eval_str_compiled(program);
        // The interpreter/VM types involved are not Send, so ship out only
        // what the assertions need.
        let _ = tx.send(result.map(|_| ()).map_err(|e| e.to_string()));
    });

    let outcome = rx.recv_timeout(std::time::Duration::from_secs(10)).expect(
        "http/serve inside async/spawn must return promptly with an error, not hang \
             the scheduler",
    );

    let err = outcome.expect_err("http/serve inside async/spawn must error, not succeed");
    assert!(
        err.contains("async/spawn"),
        "error should name async/spawn as the disallowed context, got: {err}"
    );
    assert!(
        err.to_lowercase().contains("top level"),
        "error should point the caller at the top level, got: {err}"
    );
}

// === A sibling task is unaffected: the guard's own task fails, everything
// === else in the same scheduler run proceeds normally ===
//
// Not a "sibling completes first while the op is in flight" ordering proof
// (the guard never yields — there is nothing in flight to race), but the
// property that actually matters here: one task hitting the guard must not
// wedge the scheduler for the rest of the run. `async/all` runs both tasks
// concurrently and only returns once both have settled (one rejected, one
// resolved) — a pre-guard build would never reach the `try`'s `catch` arm at
// all.
#[test]
fn http_serve_guard_does_not_stall_sibling_task() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let interp = Interpreter::new();
        let program = r#"
            (let ((out (channel/new 8)))
              (async/all
                (list
                  (async/spawn (fn ()
                    (try
                      (http/serve (fn (req) (http/ok "hi")) {:port 19940})
                      (catch e (channel/send out "serve-guard-caught")))))
                  (async/spawn (fn () (channel/send out "sibling")))))
              (list (channel/recv out) (channel/recv out)))
        "#;
        let result = interp.eval_str_compiled(program);
        let mapped = result.map_err(|e| e.to_string()).map(|v| {
            v.as_list()
                .expect("list of two channel receives")
                .iter()
                .map(|item| item.as_str().expect("string").to_string())
                .collect::<Vec<_>>()
        });
        let _ = tx.send(mapped);
    });

    let outcome = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("guarded http/serve alongside a sibling task must not hang the scheduler");

    let mut received = outcome.expect("both tasks must settle without the eval itself erroring");
    received.sort();
    assert_eq!(
        received,
        vec!["serve-guard-caught".to_string(), "sibling".to_string()],
        "both the guard-rejected task (caught) and the sibling task must complete"
    );
}

// === Sync (top-level) regression: the guard must not disturb ordinary
// === top-level arg validation, which still runs (arity error, not the
// === async-context error) since `in_async_context()` is false there ===
#[test]
fn http_serve_top_level_arity_error_unchanged() {
    let interp = Interpreter::new();
    let err = interp
        .eval_str_compiled("(http/serve)")
        .expect_err("http/serve with no args must still be an arity error at top level");
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("arity") || msg.contains("expects"),
        "top-level arity validation must be unchanged by the async-context guard, got: {msg}"
    );
}

// === Sync (top-level) regression: a real top-level http/serve still binds
// === and answers a request — the guard only fires in async context ===
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
