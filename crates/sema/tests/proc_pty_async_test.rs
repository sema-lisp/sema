//! Async-offload coverage for `proc/wait` + `pty/wait` (WP-PROCWAIT).
//!
//! Both natives block on `Child::wait()` for the child's whole lifetime
//! (`crates/sema-stdlib/src/proc.rs`, `pty.rs`). Inside `async/spawn`'d tasks
//! that would stall every sibling on the cooperative scheduler; they now
//! branch on `in_async_context()` and offload the wait through a CHECKOUT
//! registry slot (`Available`/`CheckedOut`/`Tombstone`) — see the module doc
//! comments in `proc.rs`/`pty.rs` for the full design. This suite proves the
//! scheduler-not-stalled property plus exact parity with the unchanged sync
//! path, including the "wait twice on the same handle" behavior that the sync
//! path already had (`Child::wait` caches the exit status once reaped).
//!
//! Every child process here terminates on its own within well under a
//! second (`sh -c 'exit N'` / `sh -c 'sleep 0.2; exit N'`) per the WP's
//! anti-hang rule — none read stdin or wait on a signal.

#![cfg(not(target_arch = "wasm32"))]

use sema_core::Value;
use sema_eval::Interpreter;

/// True when a real pty can be allocated, probed on a background thread with
/// a bounded wait so a pty implementation that blocks on `openpty` (rather
/// than erroring) can't hang the test suite — the probe thread itself is
/// simply abandoned (and the process exits without joining it) if it never
/// answers.
fn pty_available() -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let ok = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .is_ok();
        let _ = tx.send(ok);
    });
    rx.recv_timeout(std::time::Duration::from_secs(3))
        .unwrap_or(false)
}

// === Scheduler-not-stalled: a sibling task completes while proc/wait is in flight ===
//
// Pre-conversion, `proc/wait`'s `child.wait()` never yields, so the sibling
// (which sends immediately, no delay) can only run AFTER the whole 0.2s wait
// completes — "proc" always wins the channel race. Post-conversion
// `proc/wait` parks on `AwaitIo` the instant it's called, giving the
// scheduler a chance to run the sibling task while the child is still
// sleeping. Ordering is asserted via channel receive order — never a
// wall-clock duration assert.
#[test]
fn proc_wait_async_lets_sibling_run_first() {
    let interp = Interpreter::new();
    let program = r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (let ((h (proc/spawn (list "sh" "-c" "sleep 0.2; exit 0"))))
                  (proc/wait h)
                  (proc/close h))
                (channel/send out "proc")))
              (async/spawn (fn () (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("sibling-ordering program evaluated");
    let received: Vec<String> = result
        .as_list()
        .expect("channel receives list")
        .iter()
        .map(|v| v.as_str().expect("string value").to_string())
        .collect();
    assert_eq!(
        received.len(),
        2,
        "expected two channel receives: {received:?}"
    );
    assert_eq!(
        received,
        vec!["sibling".to_string(), "proc".to_string()],
        "sibling task must complete while proc/wait is in flight \
         (pre-conversion proc/wait always wins), got {received:?}"
    );
}

/// `proc/wait`'s exit code inside `async/spawn` matches the synchronous path.
#[test]
fn proc_wait_async_matches_sync_exit_code() {
    let interp = Interpreter::new();
    let sync_v = interp
        .eval_str_compiled(
            r#"
            (let ((h (proc/spawn (list "sh" "-c" "exit 5"))))
              (let ((code (proc/wait h))) (proc/close h) code))
            "#,
        )
        .expect("sync proc/wait");
    let async_v = interp
        .eval_str_compiled(
            r#"
            (await (async/spawn (fn ()
              (let ((h (proc/spawn (list "sh" "-c" "exit 5"))))
                (let ((code (proc/wait h))) (proc/close h) code)))))
            "#,
        )
        .expect("async proc/wait");
    assert_eq!(sync_v, async_v);
    assert_eq!(sync_v, Value::int(5));
}

/// `proc/wait` called twice (sequentially) on the same handle inside one
/// async task returns the same exit code both times — matching
/// `proc.rs::tests::double_wait_returns_same_code_sync` exactly, just through
/// the offload.
#[test]
fn proc_wait_async_double_wait_matches_sync() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(
            r#"
            (await (async/spawn (fn ()
              (let ((h (proc/spawn (list "sh" "-c" "exit 7"))))
                (let ((first (proc/wait h))
                      (second (proc/wait h)))
                  (proc/close h)
                  (list first second))))))
            "#,
        )
        .expect("double proc/wait async");
    let codes: Vec<Value> = result.as_list().expect("list").to_vec();
    assert_eq!(codes, vec![Value::int(7), Value::int(7)]);
}

/// Two sibling tasks both call `proc/wait` on the SAME handle concurrently.
/// Only one can hold the checkout at a time; the other must queue (the
/// `Acquire` phase re-attempting checkout each poll) rather than deadlock or
/// panic, and both must observe the identical exit code once the child is
/// reaped — proving the queued-caller path documented in `proc.rs`.
#[test]
fn proc_wait_async_concurrent_waiters_both_succeed() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(
            r#"
            (let ((h (proc/spawn (list "sh" "-c" "sleep 0.2; exit 3")))
                  (out (channel/new 8)))
              (async/all
                (list
                  (async/spawn (fn () (channel/send out (proc/wait h))))
                  (async/spawn (fn () (channel/send out (proc/wait h))))))
              (proc/close h)
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("concurrent proc/wait on one handle");
    let codes: Vec<Value> = result.as_list().expect("list").to_vec();
    assert_eq!(
        codes,
        vec![Value::int(3), Value::int(3)],
        "both queued waiters must see the same reaped exit code"
    );
}

// === pty/wait — same shape as proc/wait, gated on real pty availability ===

#[test]
fn pty_wait_async_lets_sibling_run_first() {
    if !pty_available() {
        eprintln!("skipping pty_wait_async_lets_sibling_run_first: no pty available");
        return;
    }
    let interp = Interpreter::new();
    let program = r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (let ((h (pty/spawn (list "sh" "-c" "sleep 0.2; exit 0"))))
                  (pty/wait h)
                  (pty/close h))
                (channel/send out "pty")))
              (async/spawn (fn () (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("sibling-ordering program evaluated");
    let received: Vec<String> = result
        .as_list()
        .expect("channel receives list")
        .iter()
        .map(|v| v.as_str().expect("string value").to_string())
        .collect();
    assert_eq!(
        received,
        vec!["sibling".to_string(), "pty".to_string()],
        "sibling task must complete while pty/wait is in flight, got {received:?}"
    );
}

#[test]
fn pty_wait_async_matches_sync_exit_code() {
    if !pty_available() {
        eprintln!("skipping pty_wait_async_matches_sync_exit_code: no pty available");
        return;
    }
    let interp = Interpreter::new();
    let sync_v = interp
        .eval_str_compiled(
            r#"
            (let ((h (pty/spawn (list "sh" "-c" "exit 5"))))
              (let ((code (pty/wait h))) (pty/close h) code))
            "#,
        )
        .expect("sync pty/wait");
    let async_v = interp
        .eval_str_compiled(
            r#"
            (await (async/spawn (fn ()
              (let ((h (pty/spawn (list "sh" "-c" "exit 5"))))
                (let ((code (pty/wait h))) (pty/close h) code)))))
            "#,
        )
        .expect("async pty/wait");
    assert_eq!(sync_v, async_v);
    assert_eq!(sync_v, Value::int(5));
}

/// `pty/wait` called twice (sequentially) on the same handle inside one async
/// task returns the same exit code both times — matches
/// `pty.rs::tests::double_wait_returns_same_code_sync` through the offload.
#[test]
fn pty_wait_async_double_wait_matches_sync() {
    if !pty_available() {
        eprintln!("skipping pty_wait_async_double_wait_matches_sync: no pty available");
        return;
    }
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(
            r#"
            (await (async/spawn (fn ()
              (let ((h (pty/spawn (list "sh" "-c" "exit 7"))))
                (let ((first (pty/wait h))
                      (second (pty/wait h)))
                  (pty/close h)
                  (list first second))))))
            "#,
        )
        .expect("double pty/wait async");
    let codes: Vec<Value> = result.as_list().expect("list").to_vec();
    assert_eq!(codes, vec![Value::int(7), Value::int(7)]);
}
