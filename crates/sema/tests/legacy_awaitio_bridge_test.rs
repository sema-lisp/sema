//! Gate for the `LegacyAwaitIoBridge`: legacy offloaded-I/O async ops that arm an
//! `IoHandle` and yield `YieldReason::AwaitIo` must become COOPERATIVE through the
//! unified runtime (`eval_str_via_runtime`) — parked on the VM thread, polled to
//! completion, and resumed — instead of blocking the VM thread.
//!
//! Exercised via `llm/complete` over a keyless, deterministic delayed
//! `FakeProvider`:
//!   1. a standalone `llm/complete` returns the provider's answer through the runtime;
//!   2. two `async/spawn`ed completions OVERLAP their offloaded jobs (peak in-flight
//!      >= 2, wall ≈ max not sum);
//!   3. `async/cancel` on a task PARKED on an `AwaitIo` handle fires the abort hook
//!      and settles the task Cancelled.
//!
//! `IO_INFLIGHT` is a process-global atomic, so these `#[serial]` tests must not
//! share a process with unrelated in-flight capture.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{
    io_peak_inflight, register_test_provider, reset_io_inflight, reset_runtime_state,
};
use sema_llm::fake::FakeProvider;
use serial_test::serial;

/// Gate 1: a top-level `(llm/complete …)` under the runtime offloads + yields
/// `AwaitIo`, the runtime bridges it to completion, and the answer is returned —
/// matching the provider's canned reply (the `eval_str` oracle).
#[test]
#[serial]
fn standalone_complete_through_runtime_returns_answer() {
    reset_io_inflight();
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("hello from fake")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let result = interp
        .eval_str_via_runtime(r#"(llm/complete "hi")"#)
        .expect("standalone llm/complete drives through the runtime AwaitIo bridge");
    assert_eq!(result.as_str(), Some("hello from fake"));
}

/// Gate 2: two `async/spawn`ed completions run their offloaded jobs SIMULTANEOUSLY
/// on the shared IO pool while the runtime polls both — proven by peak in-flight
/// >= 2 and an overlapped wall clock (≈ one delay, not two).
#[test]
#[serial]
fn two_spawned_completes_overlap_through_runtime() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    // Echo mode + a 300 ms per-call delay: correlation is deterministic and the
    // serial floor (~600 ms) is well separated from the overlapped time (~300 ms).
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(300)
        .echo()
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (let ((t0 (sys/elapsed)))
          (let ((a (async/spawn (fn () (llm/complete "p0"))))
                (b (async/spawn (fn () (llm/complete "p1")))))
            (let ((res (async/all (list a b))))
              (list res (floor (/ (- (sys/elapsed) t0) 1000000))))))
    "#;
    let result = interp
        .eval_str_via_runtime(program)
        .expect("two spawned llm/complete overlap through the runtime");
    let outer = result.as_list().expect("(results wall-ms)");
    let res = outer[0].as_list().expect("results list");
    assert_eq!(res.len(), 2, "two completions");
    assert_eq!(res[0].as_str(), Some("p0"));
    assert_eq!(res[1].as_str(), Some("p1"));

    let wall_ms = outer[1].as_int().expect("wall ms");
    assert!(
        wall_ms < 550,
        "expected overlapped wall < 550 ms (serial floor ~600 ms), got {wall_ms} ms"
    );
    assert!(
        io_peak_inflight() >= 2,
        "expected peak in-flight >= 2 (true overlap through the runtime), got {}",
        io_peak_inflight()
    );
}

/// Gate 3: `async/cancel` on a task PARKED on an `AwaitIo` handle interrupts it —
/// the abort hook fires and the task settles Cancelled, so awaiting it raises the
/// `:cancelled` condition.
#[test]
#[serial]
fn cancel_of_parked_awaitio_task_settles_cancelled() {
    reset_io_inflight();
    // A long delay guarantees the spawned completion is still PARKED on its
    // `AwaitIo` handle when the cancellation lands.
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(5000)
        .reply("should never arrive")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (let ((p (async/spawn (fn () (llm/complete "slow")))))
          (async/sleep 20)
          (async/cancel p)
          (try (await p) (catch e (:type e))))
    "#;
    let result = interp
        .eval_str_via_runtime(program)
        .expect("cancel of a parked AwaitIo task drives through the runtime");
    assert_eq!(
        result.as_keyword().as_deref(),
        Some("cancelled"),
        "awaiting a cancelled parked-AwaitIo task raises the :cancelled condition",
    );
}
