//! Gate for adversarial #7: an in-flight `llm/embed` stopped by `async/cancel`
//! with OTel enabled must NOT crash at teardown.
//!
//! When explicit cancellation stops a task parked on an offloaded `AwaitIo` future,
//! that task is left `Blocked` in the persistent thread-local scheduler. Its
//! `IoHandle` owns a detached `LlmSpan`; if the task survives to thread/process
//! teardown the span drops THEN, calling `span.end()` against an already-destructed
//! thread-local → `AccessError` → process abort.
//!
//! The fix reaps abandoned/leftover tasks during normal scheduler operation so no
//! span-owning `IoHandle` survives to teardown. These tests prove (a) the embed
//! repro returns `:caught` and the binary exits cleanly (a teardown abort would
//! SIGABRT the test process), and (b) `scheduler_task_count()` is 0 immediately
//! after a top-level await observes an explicitly cancelled in-flight task.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;
use serial_test::serial;

/// (a) The embed-under-otel repro: an `llm/embed` whose provider sleeps 300 ms,
/// explicitly cancelled after 50 ms, caught. Must return `:caught` AND leave
/// no stranded span-owning task behind (so the process exits cleanly — a teardown
/// abort would crash this very test binary). Run 3× to shake out flakiness.
#[test]
#[serial]
fn explicitly_cancelled_embed_under_otel_does_not_abort_at_teardown() {
    for _ in 0..3 {
        let _cap = sema_otel::testing::install();

        let fake = FakeProvider::builder("fake")
            .model("fake-embed")
            .embed_delay(300)
            .embed_with_tokens(vec![vec![0.1, 0.2, 0.3]], 5)
            .build();

        let interp = Interpreter::new();
        reset_runtime_state();
        register_test_provider(Box::new(fake));

        let program = r#"
            (define p (async/spawn (fn () (llm/embed "slow"))))
            (async/spawn (fn () (async/sleep 50) (async/cancel p)))
            (try (async/await p) (catch e :caught))
        "#;
        let result = interp
            .eval_str_compiled(program)
            .expect("explicitly cancelled embed program evaluated");
        assert_eq!(
            result,
            sema_core::Value::keyword("caught"),
            "explicit cancellation must surface as a caught error → :caught"
        );

        // No span-owning task may survive the run (would abort at teardown).
        assert_eq!(
            interp.runtime_live_task_count(),
            0,
            "the stranded embed task must be reaped during the run, not left for teardown"
        );
    }
}

/// (b) Keyless/deterministic task-count proof: explicitly cancelling an in-flight
/// `llm/io-sleep-once` after 50 ms (against 300 ms of work) must
/// leave the thread-local scheduler holding ZERO tasks — direct evidence the
/// stranded `AwaitIo` task was reaped.
#[test]
#[serial]
fn explicitly_cancelled_io_task_is_reaped() {
    let interp = Interpreter::new();

    let program = r#"
        (define p (async/spawn (fn () (llm/io-sleep-once 0 300))))
        (async/spawn (fn () (async/sleep 50) (async/cancel p)))
        (try (async/await p) (catch e :caught))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("explicitly cancelled io program evaluated");
    assert_eq!(result, sema_core::Value::keyword("caught"));

    assert_eq!(
        interp.runtime_live_task_count(),
        0,
        "the stranded io task must be reaped during the run"
    );
}

/// Control: without cancellation, the work completes normally with no stranded
/// task and no abort. (Mirrors the verifier's no-abort control.)
#[test]
#[serial]
fn completing_embed_under_otel_leaves_no_stranded_task() {
    let _cap = sema_otel::testing::install();

    let fake = FakeProvider::builder("fake")
        .model("fake-embed")
        .embed_delay(20)
        .embed_with_tokens(vec![vec![0.1, 0.2, 0.3]], 5)
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (try
          (embedding/length (async/await (async/spawn (fn () (llm/embed "ok")))))
          (catch e :err))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("completing embed program evaluated");
    assert_eq!(result, sema_core::Value::int(3), "embed completes → 3 dims");
    assert_eq!(interp.runtime_live_task_count(), 0, "no stranded task");
}
