//! Coverage for the public `Interpreter` host surface (P6-1 Task 3):
//! `submit_str`/`submit_str_guarded`/`submit_value`, `drive_until_settled`/`drive_turn`,
//! `take_output` (root-tagged captured output), `command_handle`, and
//! `shutdown`.

mod common;

use std::cell::Cell;
use std::time::{Duration, Instant};

use common::watchdog::run_sema_with_timeout;
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
