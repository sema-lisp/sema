//! Async-offload coverage for `proc/wait` + `pty/wait` (WP-PROCWAIT).
//!
//! Both natives block on `Child::wait()` for the child's whole lifetime
//! (`crates/sema-stdlib/src/proc.rs`, `pty.rs`). Inside `async/spawn`'d tasks
//! that would stall every sibling on the cooperative scheduler; in a runtime
//! quantum they offload the wait through the gate-guarded CHECKOUT
//! (`runtime_offload::checkout_external`) — the registry slot
//! (`Available`/`CheckedOut`/`Tombstone`) is serialized by a per-handle
//! `ResourceGate`; see the module doc comments in `proc.rs`/`pty.rs` for the
//! full design. This suite proves the scheduler-not-stalled property, exact
//! parity with the unchanged sync path (including the "wait twice on the same
//! handle" behavior — `Child::wait` caches the exit status once reaped), and
//! that a cancelled wait settles cleanly (the process-group SIGKILL abort hook
//! unsticks the blocking worker) without wedging the registry.
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
    assert_eq!(
        interp.runtime_resource_gate_count(),
        0,
        "proc/close must return the runtime's gate registry to baseline"
    );
}

#[test]
fn proc_close_from_foreign_runtime_is_offloaded_and_closes_owner_gate() {
    let owner = Interpreter::new();
    let caller = Interpreter::new();
    let handle = owner
        .eval_str_via_runtime(
            r#"
            (define foreign-close-proc
              (proc/spawn (list "sh" "-c" "exit 0")))
            (proc/wait foreign-close-proc)
            foreign-close-proc
            "#,
        )
        .expect("owner runtime creates a process gate")
        .as_int()
        .expect("proc handle is an integer");
    assert_eq!(owner.runtime_resource_gate_count(), 1);
    assert_eq!(caller.runtime_resource_gate_count(), 0);

    let result = caller
        .eval_str_via_runtime(&format!(
            r#"
            (let ((out (channel/new 2)))
              (async/all
                (list
                  (async/spawn (fn ()
                    (proc/close {handle})
                    (channel/send out :proc)))
                  (async/spawn (fn () (channel/send out :sibling)))))
              (list (channel/recv out) (channel/recv out)))
            "#
        ))
        .expect("foreign process close uses a caller-runtime External offload");
    assert_eq!(
        result.as_seq().expect("receive order"),
        &[Value::keyword("sibling"), Value::keyword("proc")],
        "the foreign terminal wait must yield the caller VM to its sibling"
    );
    assert_eq!(owner.runtime_resource_gate_count(), 0);
    assert_eq!(caller.runtime_resource_gate_count(), 0);
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

// === Cancellation through the ResourceGate + checkout_external path ===
//
// Cancelling a spawned proc chain must settle the task Cancelled (never hang or
// panic) AND leave the thread-local registry + runtime usable: a fresh proc
// spawned afterwards waits normally. Exercises the checkout continuations'
// Cancelled arms and the process-group SIGKILL abort hook.
#[test]
fn proc_cancelled_wait_settles_cancelled_and_registry_stays_usable() {
    let interp = Interpreter::new();
    let program = r#"
        (let ((h (proc/spawn (list "sh" "-c" "sleep 2; exit 0"))))
          (let ((p (async/spawn (fn () (proc/wait h)))))
            (async/cancel p)
            (let ((caught (try (async/await p) (catch e :caught))))
              (proc/close h)
              ;; a fresh handle must still work after the cancellation
              (let ((h2 (proc/spawn (list "sh" "-c" "exit 4"))))
                (let ((code (proc/wait h2)))
                  (proc/close h2)
                  (list caught code))))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("cancelled proc chain evaluates without wedging the runtime");
    let parts: Vec<Value> = result.as_list().expect("result list").to_vec();
    assert_eq!(
        parts[0],
        Value::keyword("caught"),
        "awaiting the cancelled task must raise the :cancelled condition, got {:?}",
        parts[0]
    );
    assert_eq!(
        parts[1].as_int(),
        Some(4),
        "a fresh proc handle must work after the cancellation (registry not wedged)"
    );
    assert_eq!(interp.runtime_resource_gate_count(), 0);
}

// A sibling queued behind a busy handle: `slow` holds the gate on a long
// `proc/wait`; `queued` parks FIFO behind it, then is cancelled while still
// queued. The cancelled acquirer must be removed from the gate FIFO (never
// owning the resource) and settle :cancelled. Cancelling `slow` too fires the
// process-group SIGKILL abort, unsticking the blocking wait; the whole chain
// settles cleanly and a fresh handle still works — the registry is not wedged.
#[test]
fn proc_cancel_while_queued_behind_busy_handle() {
    let interp = Interpreter::new();
    let program = r#"
        (let ((h (proc/spawn (list "sh" "-c" "sleep 2; exit 0"))))
          (let ((slow (async/spawn (fn () (proc/wait h))))
                (queued (async/spawn (fn () (proc/wait h)))))
            ;; cancel the queued waiter while `slow` holds the gate, then cancel
            ;; the gate holder — its SIGKILL abort unsticks the blocking wait.
            (async/cancel queued)
            (async/cancel slow)
            (let ((q (try (async/await queued) (catch e :q-cancelled)))
                  (s (try (async/await slow) (catch e :slow-cancelled))))
              (proc/close h)
              ;; a fresh handle must still wait normally after all that
              (let ((h2 (proc/spawn (list "sh" "-c" "exit 9"))))
                (let ((code (proc/wait h2)))
                  (proc/close h2)
                  (list q s code))))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("cancel-while-queued evaluates without hanging or panicking");
    let parts: Vec<Value> = result.as_list().expect("result list").to_vec();
    assert_eq!(
        parts[0],
        Value::keyword("q-cancelled"),
        "the queued-then-cancelled waiter must raise :cancelled, got {:?}",
        parts[0]
    );
    assert_eq!(
        parts[1],
        Value::keyword("slow-cancelled"),
        "the cancelled gate holder must raise :cancelled, got {:?}",
        parts[1]
    );
    assert_eq!(
        parts[2].as_int(),
        Some(9),
        "a fresh proc handle must work after cancelling both waiters"
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
    assert_eq!(
        interp.runtime_resource_gate_count(),
        0,
        "pty/close must return the runtime's gate registry to baseline"
    );
}

#[test]
fn pty_close_from_foreign_runtime_is_offloaded_and_closes_owner_gate() {
    if !pty_available() {
        eprintln!("skipping foreign-runtime pty close: no pty available");
        return;
    }
    let owner = Interpreter::new();
    let caller = Interpreter::new();
    let handle = owner
        .eval_str_via_runtime(
            r#"
            (define foreign-close-pty
              (pty/spawn (list "sh" "-c" "exit 0")))
            (pty/wait foreign-close-pty)
            foreign-close-pty
            "#,
        )
        .expect("owner runtime creates a pty gate")
        .as_int()
        .expect("pty handle is an integer");
    assert_eq!(owner.runtime_resource_gate_count(), 1);
    assert_eq!(caller.runtime_resource_gate_count(), 0);

    let result = caller
        .eval_str_via_runtime(&format!(
            r#"
            (let ((out (channel/new 2)))
              (async/all
                (list
                  (async/spawn (fn ()
                    (pty/close {handle})
                    (channel/send out :pty)))
                  (async/spawn (fn () (channel/send out :sibling)))))
              (list (channel/recv out) (channel/recv out)))
            "#
        ))
        .expect("foreign pty close uses a caller-runtime External offload");
    assert_eq!(
        result.as_seq().expect("receive order"),
        &[Value::keyword("sibling"), Value::keyword("pty")],
        "the foreign pty terminal wait must yield the caller VM to its sibling"
    );
    assert_eq!(owner.runtime_resource_gate_count(), 0);
    assert_eq!(caller.runtime_resource_gate_count(), 0);
}

/// Cancelling a spawned pty chain settles the task Cancelled and leaves the
/// registry usable (the pty analogue of the proc cancel test) — the process-
/// group SIGKILL abort hook unsticks the blocking wait.
#[test]
fn pty_cancelled_wait_settles_cancelled_and_registry_stays_usable() {
    if !pty_available() {
        eprintln!("skipping pty_cancelled_wait_...: no pty available");
        return;
    }
    let interp = Interpreter::new();
    let program = r#"
        (let ((h (pty/spawn (list "sh" "-c" "sleep 2; exit 0"))))
          (let ((p (async/spawn (fn () (pty/wait h)))))
            (async/cancel p)
            (let ((caught (try (async/await p) (catch e :caught))))
              (pty/close h)
              (let ((h2 (pty/spawn (list "sh" "-c" "exit 4"))))
                (let ((code (pty/wait h2)))
                  (pty/close h2)
                  (list caught code))))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("cancelled pty chain evaluates without wedging the runtime");
    let parts: Vec<Value> = result.as_list().expect("result list").to_vec();
    assert_eq!(
        parts[0],
        Value::keyword("caught"),
        "awaiting the cancelled task must raise :cancelled, got {:?}",
        parts[0]
    );
    assert_eq!(
        parts[1].as_int(),
        Some(4),
        "a fresh pty handle must work after the cancellation"
    );
    assert_eq!(interp.runtime_resource_gate_count(), 0);
}

/// A queued pty waiter cancelled while parked behind a busy handle, then the
/// gate holder cancelled too (SIGKILL abort unsticks it) — both settle
/// :cancelled and a fresh handle still works.
#[test]
fn pty_cancel_while_queued_behind_busy_handle() {
    if !pty_available() {
        eprintln!("skipping pty_cancel_while_queued_...: no pty available");
        return;
    }
    let interp = Interpreter::new();
    let program = r#"
        (let ((h (pty/spawn (list "sh" "-c" "sleep 2; exit 0"))))
          (let ((slow (async/spawn (fn () (pty/wait h))))
                (queued (async/spawn (fn () (pty/wait h)))))
            (async/cancel queued)
            (async/cancel slow)
            (let ((q (try (async/await queued) (catch e :q-cancelled)))
                  (s (try (async/await slow) (catch e :slow-cancelled))))
              (pty/close h)
              (let ((h2 (pty/spawn (list "sh" "-c" "exit 9"))))
                (let ((code (pty/wait h2)))
                  (pty/close h2)
                  (list q s code))))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("cancel-while-queued pty evaluates without hanging");
    let parts: Vec<Value> = result.as_list().expect("result list").to_vec();
    assert_eq!(parts[0], Value::keyword("q-cancelled"));
    assert_eq!(parts[1], Value::keyword("slow-cancelled"));
    assert_eq!(parts[2].as_int(), Some(9));
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
