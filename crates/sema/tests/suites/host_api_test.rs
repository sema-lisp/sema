//! Coverage for the public `Interpreter` host surface (P6-1 Task 3):
//! `submit_str`/`submit_str_guarded`/`submit_value`, `drive_until_settled`/`drive_turn`,
//! `take_output` (root-tagged captured output), `command_handle`, and
//! `shutdown`.

use std::cell::Cell;
use std::time::{Duration, Instant};

use crate::common::watchdog::run_sema_with_timeout;
use sema_eval::Interpreter;
use sema_vm::runtime::{DriveState, OutputEvent, RootOptions, RootPoll, ShutdownOptions};

/// Two roots submitted with `capture_output: true`, each printing two
/// markers around an `async/sleep`, must have their output tagged with the
/// SUBMITTING root — never cross-attributed to the other root — even though
/// the runtime interleaves them (the shorter sleep lets root B's second
/// print land between root A's two prints).
#[test]
fn captured_output_tags_the_correct_root_when_interleaved() {
    let interp = Interpreter::new();

    let handle_a = interp
        .submit_str(
            r#"(println "A1") (async/sleep 5) (println "A2")"#,
            RootOptions {
                name: Some("root-a".into()),
                capture_output: true,
            },
        )
        .expect("root A submits");
    let handle_b = interp
        .submit_str(
            r#"(println "B1") (async/sleep 1) (println "B2")"#,
            RootOptions {
                name: Some("root-b".into()),
                capture_output: true,
            },
        )
        .expect("root B submits");

    // Root B's shorter sleep means it settles first; driving A to
    // settlement drives the whole runtime (fair scheduling), so B's events
    // land in between A's two prints.
    interp
        .drive_until_settled(&handle_a)
        .expect("root A settles");
    interp
        .drive_until_settled(&handle_b)
        .expect("root B settles");

    let events = interp.take_output();
    assert!(
        !events.is_empty(),
        "expected captured output from both roots"
    );

    let text_for = |root: sema_core::runtime::RootId| -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                OutputEvent::Stdout { root: r, text } if *r == root => Some(text.clone()),
                _ => None,
            })
            .collect()
    };

    let a_lines: String = text_for(handle_a.id()).concat();
    let b_lines: String = text_for(handle_b.id()).concat();
    assert!(
        a_lines.contains("A1"),
        "root A missing its A1 marker: {a_lines:?}"
    );
    assert!(
        a_lines.contains("A2"),
        "root A missing its A2 marker: {a_lines:?}"
    );
    assert!(
        !a_lines.contains('B'),
        "root A output contaminated with B: {a_lines:?}"
    );
    assert!(
        b_lines.contains("B1"),
        "root B missing its B1 marker: {b_lines:?}"
    );
    assert!(
        b_lines.contains("B2"),
        "root B missing its B2 marker: {b_lines:?}"
    );
    assert!(
        !b_lines.contains('A'),
        "root B output contaminated with A: {b_lines:?}"
    );

    // Every captured event is tagged with one of the two submitted roots —
    // nothing leaked in from an untracked source.
    for event in &events {
        let root = match event {
            OutputEvent::Stdout { root, .. } | OutputEvent::Stderr { root, .. } => *root,
        };
        assert!(
            root == handle_a.id() || root == handle_b.id(),
            "captured event tagged with an unexpected root: {event:?}"
        );
    }

    // Second drain is empty — `take_output` actually drains, it doesn't peek.
    assert!(interp.take_output().is_empty());
}

/// A native reached through a structural callback runs between VM quanta. It still
/// belongs to the invoking root, so output capture must see the same published root
/// identity as an ordinary native call inside the VM quantum.
#[test]
fn captured_output_includes_direct_native_callbacks() {
    let interp = Interpreter::new();
    let handle = interp
        .submit_str(
            r#"(apply println (list "native-callback-output"))"#,
            RootOptions {
                name: Some("native-callback-root".into()),
                capture_output: true,
            },
        )
        .expect("root submits");

    interp.drive_until_settled(&handle).expect("root settles");
    let events = interp.take_output();
    assert!(
        events.iter().any(|event| {
            matches!(
                event,
                OutputEvent::Stdout { root, text }
                    if *root == handle.id() && text.contains("native-callback-output")
            )
        }),
        "direct native callback output was not attributed to its root: {events:?}"
    );
}

/// Root mains own dynamic scopes just like spawned tasks. A budget opened by root A
/// before it parks must not become ambient input to independently-submitted root B.
#[test]
fn independently_submitted_roots_isolate_llm_scopes() {
    let interp = Interpreter::new();
    let root_a = interp
        .submit_str(
            r#"
            (llm/with-budget
              {:max-tokens 7}
              (fn ()
                (async/sleep 25)
                (:token-limit (llm/budget-remaining))))
            "#,
            RootOptions::default(),
        )
        .expect("budgeted root submits");
    let root_b = interp
        .submit_str("(llm/budget-remaining)", RootOptions::default())
        .expect("unscoped root submits");

    let a = interp
        .drive_until_settled(&root_a)
        .expect("budgeted root settles");
    let b = interp
        .drive_until_settled(&root_b)
        .expect("unscoped root settles");

    assert_eq!(a, sema_core::Value::int(7));
    assert!(
        b.is_nil(),
        "root B inherited root A's active LLM budget scope: {b}"
    );
}

/// submit -> drive_turn loop -> poll_result -> shutdown report counts.
#[test]
fn lifecycle_submit_drive_turn_poll_shutdown() {
    let interp = Interpreter::new();

    let handle = interp
        .submit_str("(+ 1 2)", RootOptions::default())
        .expect("submit succeeds");

    // Drive bounded turns (never blocking) until the root settles or we've
    // spent an unreasonable number of turns — the program is a single
    // synchronous addition, so this must resolve in very few turns.
    let mut turns = 0;
    loop {
        if matches!(handle.poll_result(), RootPoll::Ready(_)) {
            break;
        }
        turns += 1;
        assert!(
            turns < 10_000,
            "root never settled after {turns} drive turns"
        );
        match interp.drive_turn().expect("drive turn succeeds") {
            DriveState::Progress { .. } => {}
            DriveState::Idle {
                next_deadline: Some(deadline),
                ..
            } => {
                let now = Instant::now();
                if deadline > now {
                    std::thread::sleep(deadline - now);
                }
            }
            other => panic!("unexpected idle drive state for a synchronous program: {other:?}"),
        }
    }

    match handle.poll_result() {
        RootPoll::Ready(settlement) => match &settlement.outcome {
            sema_core::runtime::TaskOutcome::Returned(value) => {
                assert_eq!(*value, sema_core::Value::int(3));
            }
            other => panic!("expected the root to return 3, got {other:?}"),
        },
        _ => panic!("expected the root to have settled, got a non-ready poll instead"),
    }

    drop(handle);

    let report = interp
        .shutdown(ShutdownOptions {
            deadline: Instant::now() + Duration::from_secs(5),
            drive_budget: sema_vm::runtime::DriveBudget::host_default(),
        })
        .expect("shutdown succeeds");
    assert!(report.clean, "expected a clean shutdown: {report:?}");
    assert_eq!(report.live_roots, 0);
    assert_eq!(report.live_tasks, 0);
    assert_eq!(report.active_waits, 0);
}

/// `submit_str` with a parse error surfaces a `SemaError` without poisoning
/// the runtime — a subsequent submit on the same interpreter still works.
#[test]
fn parse_error_does_not_poison_the_runtime() {
    let interp = Interpreter::new();

    let err = match interp.submit_str("(+ 1 2", RootOptions::default()) {
        Err(e) => e,
        Ok(_) => panic!("unbalanced parens must fail to parse"),
    };
    let _ = err.to_string(); // surfaced as a real SemaError, not a panic

    let handle = interp
        .submit_str("(+ 1 2)", RootOptions::default())
        .expect("a later submit on the same interpreter still works");
    let result = interp
        .drive_until_settled(&handle)
        .expect("subsequent submit drives to completion");
    assert_eq!(result, sema_core::Value::int(3));
}

/// A host admission check that protects re-entrant macro expansion runs after
/// expansion has completed but before the prepared VM becomes a runtime root.
#[test]
fn guarded_submit_checks_admission_after_expansion_before_root_creation() {
    let interp = Interpreter::new();
    let guard_saw_macro = Cell::new(false);

    let error = match interp.submit_str_guarded(
        "(defmacro guarded-submit-macro () 7)\n(define guarded-submit-root-ran 1)",
        RootOptions::default(),
        || {
            guard_saw_macro.set(
                interp
                    .global_env
                    .get(sema_core::intern("guarded-submit-macro"))
                    .is_some(),
            );
            Err(sema_core::SemaError::eval("host admission changed"))
        },
    ) {
        Err(error) => error,
        Ok(_) => panic!("the host guard must reject before root submission"),
    };

    assert!(guard_saw_macro.get(), "guard ran before macro expansion");
    assert!(error.to_string().contains("host admission changed"));
    assert!(
        interp
            .global_env
            .get(sema_core::intern("guarded-submit-root-ran"))
            .is_none(),
        "rejected prepared VM must not execute"
    );
}

/// A root submitted WITHOUT `capture_output` still writes straight to
/// process stdout, exactly as every existing eval entry point does — proven
/// out-of-process since in-process capture would require redirecting the
/// real file descriptor.
#[test]
fn non_captured_output_still_reaches_process_stdout() {
    let run = run_sema_with_timeout(
        r#"(println "not-captured-output")"#,
        Duration::from_secs(10),
    );
    assert!(run.status.success(), "run failed; stderr:\n{}", run.stderr);
    assert!(
        run.stdout.contains("not-captured-output"),
        "expected the marker on real stdout; got stdout:\n{}\nstderr:\n{}",
        run.stdout,
        run.stderr
    );
}

/// Closing a channel that has no parked sender or receiver must not leave a
/// pending stage behind that `drive_roots` can never drain.
///
/// `drive_roots` (the selected-roots host surface the WASM Promise driver in
/// `crates/sema-wasm/src/driver.rs` uses) only advances pending stages that
/// belong to one of its roots. A wake batch with no waiters belongs to no
/// task at all, so nothing would ever advance it — and `fire_timer`'s gate is
/// runtime-wide ("no ready task, no pending stage"), so one orphaned stage
/// stalls EVERY later timer on that runtime: the sleep below never fires and
/// the root never settles. Regression: the playground's `worker-pool.sema`
/// example (buffered queue, closed before any worker parks on it, then a
/// cooperative `(async/sleep 0)` yield) hung forever in the browser while
/// running fine under the native `drive`, which drains `pending` regardless
/// of ownership.
#[test]
fn closing_an_unwaited_channel_leaves_no_undrainable_pending_stage() {
    let interp = Interpreter::new();
    let handle = interp
        .submit_str(
            r#"(define jobs (channel/new 8))
               (for-each (fn (c) (channel/send jobs c)) (list 1 2 3))
               (channel/close jobs)
               (define w (async (let loop ((load 0))
                 (async/sleep 0)
                 (let ((cost (channel/recv jobs)))
                   (if (nil? cost) load (loop (+ load cost)))))))
               (await w)"#,
            RootOptions {
                name: Some("pool".into()),
                capture_output: true,
            },
        )
        .expect("root submits");
    let root = handle.id();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() < deadline,
            "root never settled under drive_roots — an undrainable pending \
             stage is blocking the timer queue"
        );
        let state = interp.drive_roots(&[root]).expect("drive turn");
        match handle.poll_result() {
            RootPoll::Pending => {}
            RootPoll::Ready(settlement) => {
                let outcome = format!("{:?}", settlement.outcome);
                assert!(
                    outcome.contains('6'),
                    "expected the three jobs (1+2+3) to be drained, got {outcome}"
                );
                return;
            }
            RootPoll::Aborted(fault) => panic!("root aborted: {fault:?}"),
            RootPoll::RuntimeDropped => panic!("runtime dropped"),
            RootPoll::InvariantViolation => panic!("invariant violation"),
        }
        // Mirror the Promise driver: wait out a reported timer deadline
        // instead of busy-polling, so a never-firing timer hangs here exactly
        // as it does in the browser.
        if let DriveState::Idle {
            next_deadline: Some(_),
            ..
        } = state
        {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// A detached task still parked on a timer when its root settles must not
/// wedge the NEXT root's timers.
///
/// Parked detached tasks deliberately outlive their root (see the drain
/// comment in `Interpreter::drive_handle_to_settlement`), so the orphan below
/// is expected to survive. What must not happen is the orphan's overdue timer
/// blocking a later root: `drive_roots` scopes `next_deadline` and
/// `inbox_wakeup_required` to its selection, so the timer wheel has to be
/// scoped the same way. Otherwise the second program's `(async/sleep 0)` never
/// fires and the root hangs forever — the playground symptom where running
/// `timeout.sema` (whose `async/timeout`/`async/race` losers are left parked)
/// poisoned every later sleep in that worker session.
#[test]
fn an_orphaned_timer_from_a_settled_root_does_not_stall_a_later_root() {
    let interp = Interpreter::new();

    // Program 1: settles immediately, leaving a detached task parked on a timer.
    let first = interp
        .submit_str(
            r#"(define orphan (async (begin (async/sleep 400) (println "orphan woke"))))
               1"#,
            RootOptions {
                name: Some("leaves-an-orphan".into()),
                capture_output: true,
            },
        )
        .expect("first root submits");
    drive_selected_to_settlement(&interp, &first, "first");

    // No driving at all while the orphan's deadline passes — what a browser
    // does between two Run clicks. The orphan timer is now overdue, and (being
    // the oldest) it is the earliest entry in the timer wheel.
    std::thread::sleep(Duration::from_millis(600));

    // Program 2: a plain cooperative yield. Its timer must still fire.
    let second = interp
        .submit_str(
            "(async/sleep 0) 7",
            RootOptions {
                name: Some("later-sleeper".into()),
                capture_output: true,
            },
        )
        .expect("second root submits");
    let outcome = drive_selected_to_settlement(&interp, &second, "second");
    assert!(
        outcome.contains('7'),
        "second root settled with the wrong value: {outcome}"
    );
}

/// Drive `handle` to settlement through `drive_roots` only — the selected-roots
/// host surface the WASM Promise driver uses — waiting out reported deadlines
/// instead of busy-polling, so a timer that never fires hangs here exactly as
/// it does in the browser. Returns the settled outcome, rendered.
fn drive_selected_to_settlement(
    interp: &Interpreter,
    handle: &sema_vm::runtime::RootHandle,
    label: &str,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() < deadline,
            "{label} root never settled under drive_roots — its timer never fired"
        );
        let state = interp.drive_roots(&[handle.id()]).expect("drive turn");
        match handle.poll_result() {
            RootPoll::Pending => {}
            RootPoll::Ready(settlement) => return format!("{:?}", settlement.outcome),
            RootPoll::Aborted(fault) => panic!("{label} root aborted: {fault:?}"),
            RootPoll::RuntimeDropped => panic!("{label} runtime dropped"),
            RootPoll::InvariantViolation => panic!("{label} invariant violation"),
        }
        if let DriveState::Idle { .. } = state {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
