//! Unified-runtime contracts for deferred Unix signal callbacks.
//!
//! The OS handler itself only records a process event. These tests exercise the
//! interpreter-owned subscription and the task-private structural callback
//! chain that `sys/check-signals` drives later on the VM thread.

#![cfg(unix)]

use std::sync::{Mutex, MutexGuard};

use sema_core::runtime::{CancelReason, TaskOutcome};
use sema_core::Value;
use sema_eval::Interpreter;
use sema_vm::runtime::{DriveState, RootOptions, RootPoll};

static SIGNAL_TEST_LOCK: Mutex<()> = Mutex::new(());

fn signal_test_guard() -> MutexGuard<'static, ()> {
    SIGNAL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn mark_sigwinch_pending() {
    sema_stdlib::mark_sigwinch_pending_for_test();
}

fn eval(interp: &Interpreter, source: &str) -> Value {
    interp
        .eval_str(source)
        .unwrap_or_else(|error| panic!("eval failed for {source:?}: {error}"))
}

fn drive_until_idle(interp: &Interpreter) {
    for _ in 0..32 {
        if matches!(
            interp.drive_turn().expect("runtime drive succeeds"),
            DriveState::Idle { .. }
        ) {
            return;
        }
    }
    panic!("runtime did not become idle within the drive budget");
}

fn drive_until_settled(interp: &Interpreter, root: &sema_vm::runtime::RootHandle) {
    for _ in 0..32 {
        if !matches!(root.poll_result(), RootPoll::Pending) {
            return;
        }
        interp.drive_turn().expect("runtime drive succeeds");
    }
    panic!("root did not settle within the drive budget");
}

#[test]
fn checking_one_interpreter_does_not_dispatch_another_interpreters_callbacks() {
    let _guard = signal_test_guard();
    let left = Interpreter::new();
    let right = Interpreter::new();
    eval(
        &left,
        "(begin
           (define signal-events '())
           (sys/on-signal :winch
             (fn () (set! signal-events (append signal-events '(:left))))))",
    );
    eval(
        &right,
        "(begin
           (define signal-events '())
           (sys/on-signal :winch
             (fn () (set! signal-events (append signal-events '(:right))))))",
    );

    mark_sigwinch_pending();
    assert_eq!(eval(&left, "(sys/check-signals)"), Value::nil());
    assert_eq!(eval(&left, "signal-events"), eval(&left, "'(:left)"));
    assert_eq!(
        eval(&right, "signal-events"),
        Value::list(Vec::new()),
        "the right subscription remains pending until the right interpreter checks"
    );

    assert_eq!(eval(&right, "(sys/check-signals)"), Value::nil());
    assert_eq!(eval(&right, "signal-events"), eval(&right, "'(:right)"));
}

#[test]
fn callbacks_keep_registration_order_and_signal_events_coalesce() {
    let _guard = signal_test_guard();
    let interp = Interpreter::new();
    eval(
        &interp,
        "(begin
           (define signal-events '())
           (sys/on-signal :winch
             (fn () (set! signal-events (append signal-events '(:one)))))
           (sys/on-signal :winch
             (fn () (set! signal-events (append signal-events '(:two))))))",
    );

    mark_sigwinch_pending();
    mark_sigwinch_pending();
    assert_eq!(eval(&interp, "(sys/check-signals)"), Value::nil());
    assert_eq!(
        eval(&interp, "signal-events"),
        eval(&interp, "'(:one :two)")
    );
    assert_eq!(eval(&interp, "(sys/check-signals)"), Value::nil());
    assert_eq!(
        eval(&interp, "signal-events"),
        eval(&interp, "'(:one :two)")
    );

    mark_sigwinch_pending();
    assert_eq!(eval(&interp, "(sys/check-signals)"), Value::nil());
    assert_eq!(
        eval(&interp, "signal-events"),
        eval(&interp, "'(:one :two :one :two)")
    );
}

#[test]
fn signal_callback_suspends_while_a_sibling_releases_it() {
    let _guard = signal_test_guard();
    let interp = Interpreter::new();
    eval(
        &interp,
        "(begin
           (define signal-gate (channel/new 1))
           (define signal-events '())
           (sys/on-signal :winch
             (fn ()
               (set! signal-events (append signal-events '(:before)))
               (channel/recv signal-gate)
               (set! signal-events (append signal-events '(:after)))))
           (async/spawn
             (fn ()
               (async/sleep 1)
               (channel/send signal-gate :released))))",
    );

    mark_sigwinch_pending();
    assert_eq!(eval(&interp, "(sys/check-signals)"), Value::nil());
    assert_eq!(
        eval(&interp, "signal-events"),
        eval(&interp, "'(:before :after)")
    );
}

#[test]
fn callback_mutation_reaches_its_still_parked_defining_frame() {
    let _guard = signal_test_guard();
    let interp = Interpreter::new();
    eval(&interp, "(define installer-gate (channel/new 1))");
    let installer = interp
        .submit_str(
            "(let ((captured 40))
               (sys/on-signal :winch
                 (fn () (set! captured (+ captured 2))))
               (channel/recv installer-gate)
               captured)",
            RootOptions::default(),
        )
        .expect("installer root admitted");
    drive_until_idle(&interp);
    assert!(matches!(installer.poll_result(), RootPoll::Pending));

    mark_sigwinch_pending();
    assert_eq!(eval(&interp, "(sys/check-signals)"), Value::nil());
    assert_eq!(
        eval(&interp, "(channel/send installer-gate :go)"),
        Value::nil()
    );

    let RootPoll::Ready(settlement) = installer.poll_result() else {
        panic!("installer root settled after its gate was released")
    };
    assert!(
        matches!(settlement.outcome, TaskOutcome::Returned(ref value) if *value == Value::int(42))
    );
}

#[test]
fn callback_failure_is_fail_fast_and_consumes_the_signal_batch() {
    let _guard = signal_test_guard();
    let interp = Interpreter::new();
    eval(
        &interp,
        "(begin
           (define signal-events '())
           (sys/on-signal :winch
             (fn () (set! signal-events (append signal-events '(:one)))))
           (sys/on-signal :winch (fn () (error \"signal boom\")))
           (sys/on-signal :winch
             (fn () (set! signal-events (append signal-events '(:three))))))",
    );

    mark_sigwinch_pending();
    let error = interp
        .eval_str("(sys/check-signals)")
        .expect_err("the second callback fails the check");
    assert!(error.to_string().contains("signal boom"));
    assert_eq!(eval(&interp, "signal-events"), eval(&interp, "'(:one)"));

    assert_eq!(eval(&interp, "(sys/check-signals)"), Value::nil());
    assert_eq!(eval(&interp, "signal-events"), eval(&interp, "'(:one)"));
}

#[test]
fn cancelling_a_parked_signal_callback_consumes_only_that_root() {
    let _guard = signal_test_guard();
    let interp = Interpreter::new();
    let other = Interpreter::new();
    eval(
        &interp,
        "(begin
           (define never-released (channel/new 1))
           (define callback-count 0)
           (sys/on-signal :winch
             (fn ()
               (set! callback-count (+ callback-count 1))
               (channel/recv never-released))))",
    );
    eval(
        &other,
        "(begin
           (define other-callback-count 0)
           (sys/on-signal :winch
             (fn () (set! other-callback-count (+ other-callback-count 1)))))",
    );

    mark_sigwinch_pending();
    let check = interp
        .submit_str("(sys/check-signals)", RootOptions::default())
        .expect("signal-check root admitted");
    drive_until_idle(&interp);
    assert!(matches!(check.poll_result(), RootPoll::Pending));

    assert!(check.cancel(CancelReason::Explicit));
    drive_until_settled(&interp, &check);
    assert!(matches!(
        check.poll_result(),
        RootPoll::Ready(settlement)
            if matches!(settlement.outcome, TaskOutcome::Cancelled(CancelReason::Explicit))
    ));
    assert_eq!(eval(&interp, "callback-count"), Value::int(1));
    assert_eq!(eval(&interp, "(sys/check-signals)"), Value::nil());
    assert_eq!(eval(&interp, "callback-count"), Value::int(1));
    assert_eq!(eval(&other, "other-callback-count"), Value::int(0));
    assert_eq!(eval(&other, "(sys/check-signals)"), Value::nil());
    assert_eq!(eval(&other, "other-callback-count"), Value::int(1));
}
