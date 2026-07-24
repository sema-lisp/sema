//! Unified-runtime contracts for deferred Unix signal callbacks.
//!
//! The OS handler itself only records a process event. These tests exercise the
//! interpreter-owned subscription and the task-private structural callback
//! chain that `sys/check-signals` drives later on the VM thread.

#![cfg(unix)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use sema_core::runtime::{CancelReason, TaskOutcome};
use sema_core::Value;
use sema_eval::Interpreter;
use sema_vm::runtime::{DriveState, RootOptions, RootPoll};

static SIGNAL_TEST_LOCK: Mutex<()> = Mutex::new(());
static PRIOR_SIGWINCH_CALLS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn prior_sigwinch_handler(_: libc::c_int) {
    PRIOR_SIGWINCH_CALLS.fetch_add(1, Ordering::Relaxed);
}

struct SigactionRestore {
    signal: libc::c_int,
    previous: libc::sigaction,
}

impl Drop for SigactionRestore {
    fn drop(&mut self) {
        // SAFETY: `previous` was returned by `sigaction` for this exact signal
        // in the same process and remains valid for process lifetime.
        let restored =
            unsafe { libc::sigaction(self.signal, &self.previous, std::ptr::null_mut()) };
        assert_eq!(restored, 0, "restore test signal disposition");
    }
}

fn install_prior_sigwinch_handler() -> SigactionRestore {
    PRIOR_SIGWINCH_CALLS.store(0, Ordering::Relaxed);
    // SAFETY: zero is a valid initial representation for `sigaction`; the mask
    // is initialized before installation and the handler has the required C ABI.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = prior_sigwinch_handler as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        assert_eq!(libc::sigemptyset(&mut action.sa_mask), 0);
        assert_eq!(libc::sigaddset(&mut action.sa_mask, libc::SIGTERM), 0);

        let mut previous: libc::sigaction = std::mem::zeroed();
        assert_eq!(
            libc::sigaction(libc::SIGWINCH, &action, &mut previous),
            0,
            "install prior SIGWINCH disposition"
        );
        SigactionRestore {
            signal: libc::SIGWINCH,
            previous,
        }
    }
}

fn current_sigwinch_action() -> libc::sigaction {
    // SAFETY: a null new-action pointer queries the current disposition, and
    // `current` points to writable storage for the result.
    unsafe {
        let mut current: libc::sigaction = std::mem::zeroed();
        assert_eq!(
            libc::sigaction(libc::SIGWINCH, std::ptr::null(), &mut current),
            0,
            "query SIGWINCH disposition"
        );
        current
    }
}

fn raise_sigwinch() {
    // SAFETY: SIGWINCH is a valid signal and these tests always install a
    // handler before raising it, so it cannot take the process default action.
    assert_eq!(unsafe { libc::raise(libc::SIGWINCH) }, 0);
}

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

#[test]
fn dropping_last_registry_restores_exact_prior_sigaction_after_gc_sever() {
    let _guard = signal_test_guard();
    let _restore = install_prior_sigwinch_handler();

    let weak_bindings = {
        let interp = Interpreter::new();
        eval(
            &interp,
            "(begin
               (define signal-root :kept-until-drop)
               (sys/on-signal :winch (fn () signal-root)))",
        );
        std::rc::Rc::downgrade(&interp.global_env.bindings)
    };
    assert_eq!(
        weak_bindings.strong_count(),
        0,
        "teardown severs the registry callback cycle"
    );

    let restored = current_sigwinch_action();
    assert_eq!(
        restored.sa_sigaction, prior_sigwinch_handler as *const () as libc::sighandler_t,
        "last registry drop restores the prior handler"
    );
    assert_ne!(
        restored.sa_flags & libc::SA_RESTART,
        0,
        "last registry drop restores the prior flags"
    );
    // SAFETY: `restored.sa_mask` was initialized by the successful sigaction
    // query above and SIGTERM is a valid signal number.
    assert_eq!(
        unsafe { libc::sigismember(&restored.sa_mask, libc::SIGTERM) },
        1
    );

    raise_sigwinch();
    assert_eq!(PRIOR_SIGWINCH_CALLS.load(Ordering::Relaxed), 1);
}

#[test]
fn dropping_one_of_two_registries_keeps_the_shared_handler_installed() {
    let _guard = signal_test_guard();
    let _restore = install_prior_sigwinch_handler();
    let left = Interpreter::new();
    let right = Interpreter::new();
    eval(&left, "(sys/on-signal :winch (fn () nil))");
    eval(
        &right,
        "(begin
           (define right-signal-count 0)
           (sys/on-signal :winch
             (fn () (set! right-signal-count (+ right-signal-count 1)))))",
    );

    drop(left);
    raise_sigwinch();
    assert_eq!(
        PRIOR_SIGWINCH_CALLS.load(Ordering::Relaxed),
        0,
        "the prior handler stays displaced while another registry owns the signal"
    );
    assert_eq!(eval(&right, "(sys/check-signals)"), Value::nil());
    assert_eq!(eval(&right, "right-signal-count"), Value::int(1));

    drop(right);
    raise_sigwinch();
    assert_eq!(
        PRIOR_SIGWINCH_CALLS.load(Ordering::Relaxed),
        1,
        "the final registry drop restores the prior handler"
    );
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
fn subscription_registered_by_one_root_is_checked_by_another_root() {
    let _guard = signal_test_guard();
    let interp = Interpreter::new();
    eval(&interp, "(define cross-root-events '())");

    let registration = interp
        .submit_str(
            "(begin
               (sys/on-signal :winch
                 (fn ()
                   (set! cross-root-events
                     (append cross-root-events '(:delivered)))))
               :registered)",
            RootOptions::default(),
        )
        .expect("registration root admitted");
    drive_until_settled(&interp, &registration);
    assert!(matches!(
        registration.poll_result(),
        RootPoll::Ready(settlement)
            if matches!(settlement.outcome, TaskOutcome::Returned(ref value)
                if *value == Value::keyword("registered"))
    ));

    mark_sigwinch_pending();
    let check = interp
        .submit_str("(sys/check-signals)", RootOptions::default())
        .expect("signal-check root admitted");
    drive_until_settled(&interp, &check);
    assert!(matches!(
        check.poll_result(),
        RootPoll::Ready(settlement)
            if matches!(settlement.outcome, TaskOutcome::Returned(ref value)
                if *value == Value::nil())
    ));
    assert_eq!(
        eval(&interp, "cross-root-events"),
        eval(&interp, "'(:delivered)")
    );
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
fn captured_frame_subscription_releases_process_handler_at_teardown() {
    let _guard = signal_test_guard();
    let _restore = install_prior_sigwinch_handler();
    {
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

    let restored = current_sigwinch_action();
    assert_eq!(
        restored.sa_sigaction, prior_sigwinch_handler as *const () as libc::sighandler_t,
        "captured-frame interpreter teardown restores the prior handler"
    );
    raise_sigwinch();
    assert_eq!(PRIOR_SIGWINCH_CALLS.load(Ordering::Relaxed), 1);

    {
        let interp = Interpreter::new();
        eval(
            &interp,
            "(begin
               (define replacement-signal-count 0)
               (sys/on-signal :winch
                 (fn ()
                   (set! replacement-signal-count
                     (+ replacement-signal-count 1)))))",
        );
        assert_ne!(
            current_sigwinch_action().sa_sigaction,
            prior_sigwinch_handler as *const () as libc::sighandler_t,
            "a fresh first subscriber installs the Sema handler"
        );
        raise_sigwinch();
        assert_eq!(
            PRIOR_SIGWINCH_CALLS.load(Ordering::Relaxed),
            1,
            "the fresh subscription owns the process handler"
        );
        assert_eq!(eval(&interp, "(sys/check-signals)"), Value::nil());
        assert_eq!(eval(&interp, "replacement-signal-count"), Value::int(1));
    }

    assert_eq!(
        current_sigwinch_action().sa_sigaction,
        prior_sigwinch_handler as *const () as libc::sighandler_t,
        "the replacement registry also releases the final process lease"
    );
    raise_sigwinch();
    assert_eq!(PRIOR_SIGWINCH_CALLS.load(Ordering::Relaxed), 2);
}

#[test]
fn retained_environment_does_not_retain_process_signal_ownership() {
    let _guard = signal_test_guard();
    let _restore = install_prior_sigwinch_handler();

    let retained_env = {
        let interp = Interpreter::new();
        eval(
            &interp,
            "(begin
               (define retained-signal-count 0)
               (sys/on-signal :winch
                 (fn ()
                   (set! retained-signal-count
                     (+ retained-signal-count 1)))))",
        );
        std::rc::Rc::clone(&interp.global_env)
    };

    let restored = current_sigwinch_action();
    assert_eq!(
        restored.sa_sigaction, prior_sigwinch_handler as *const () as libc::sighandler_t,
        "interpreter teardown restores the prior handler while its env survives"
    );
    assert_ne!(
        restored.sa_flags & libc::SA_RESTART,
        0,
        "interpreter teardown restores the prior flags"
    );
    // SAFETY: the successful sigaction query initialized `restored.sa_mask`.
    assert_eq!(
        unsafe { libc::sigismember(&restored.sa_mask, libc::SIGTERM) },
        1,
        "interpreter teardown restores the prior signal mask"
    );

    raise_sigwinch();
    assert_eq!(PRIOR_SIGWINCH_CALLS.load(Ordering::Relaxed), 1);

    let retained_env_guard = std::rc::Rc::clone(&retained_env);
    let resumed_ctx = {
        let fresh = Interpreter::new();
        std::rc::Rc::clone(&fresh.ctx)
    };
    let resumed = Interpreter::from_parts(retained_env, resumed_ctx);
    assert_eq!(eval(&resumed, "(sys/check-signals)"), Value::nil());
    assert_eq!(eval(&resumed, "retained-signal-count"), Value::int(0));

    eval(
        &resumed,
        "(sys/on-signal :winch
           (fn ()
             (set! retained-signal-count
               (+ retained-signal-count 10))))",
    );
    assert_ne!(
        current_sigwinch_action().sa_sigaction,
        prior_sigwinch_handler as *const () as libc::sighandler_t,
        "the retained builtin reacquires process ownership in its new interpreter"
    );
    raise_sigwinch();
    assert_eq!(
        PRIOR_SIGWINCH_CALLS.load(Ordering::Relaxed),
        1,
        "the resumed interpreter owns the process handler"
    );
    assert_eq!(eval(&resumed, "(sys/check-signals)"), Value::nil());
    assert_eq!(
        eval(&resumed, "retained-signal-count"),
        Value::int(10),
        "teardown cleared the old callback before the retained builtin registered a new one"
    );

    drop(resumed);
    assert_eq!(
        current_sigwinch_action().sa_sigaction,
        prior_sigwinch_handler as *const () as libc::sighandler_t,
        "the resumed interpreter releases ownership while the env still survives"
    );
    raise_sigwinch();
    assert_eq!(PRIOR_SIGWINCH_CALLS.load(Ordering::Relaxed), 2);
    drop(retained_env_guard);
}

#[test]
fn unwinding_interpreter_drop_releases_signal_ownership_with_retained_state() {
    let _guard = signal_test_guard();
    let _restore = install_prior_sigwinch_handler();
    let mut retained_env = None;
    let mut retained_ctx = None;

    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let interp = Interpreter::new();
        eval(&interp, "(sys/on-signal :winch (fn () nil))");
        retained_env = Some(std::rc::Rc::clone(&interp.global_env));
        retained_ctx = Some(std::rc::Rc::clone(&interp.ctx));
        panic!("trigger interpreter teardown during unwind");
    }));
    assert!(unwind.is_err(), "the fixture panic is caught by the host");

    let restored = current_sigwinch_action();
    assert_eq!(
        restored.sa_sigaction, prior_sigwinch_handler as *const () as libc::sighandler_t,
        "unwinding teardown restores the prior handler while env and ctx survive"
    );
    assert_ne!(
        restored.sa_flags & libc::SA_RESTART,
        0,
        "unwinding teardown restores the prior flags"
    );
    // SAFETY: the successful sigaction query initialized `restored.sa_mask`.
    assert_eq!(
        unsafe { libc::sigismember(&restored.sa_mask, libc::SIGTERM) },
        1,
        "unwinding teardown restores the prior signal mask"
    );

    raise_sigwinch();
    assert_eq!(PRIOR_SIGWINCH_CALLS.load(Ordering::Relaxed), 1);
    drop(retained_ctx);
    drop(retained_env);
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
