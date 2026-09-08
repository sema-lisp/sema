//! Concurrency gate for the unified runtime's real thread-pool executor
//! (Task 05 foundation). `sleep`, under the runtime, is a genuinely-blocking
//! external operation submitted to the executor and run on a worker thread.
//! Two `async/spawn`ed sleeps must therefore OVERLAP — total wall-time is
//! ~one sleep, not two — proving the executor + inbox-wakeup drive resumes a
//! task when its worker completes. Driven through `eval_str_via_runtime`.

use std::time::{Duration, Instant};

use sema_core::Value;
use sema_eval::Interpreter;

const SLEEP_MS: u64 = 200;

/// Two spawned blocking sleeps run concurrently on separate workers: wall-time
/// is close to a single sleep, well under the ~2x a serial execution would take.
#[test]
fn spawned_blocking_sleeps_overlap_on_executor() {
    let interp = Interpreter::new();
    let src = format!(
        r#"
        (let ((a (async/spawn (fn () (sleep {SLEEP_MS}) 1)))
              (b (async/spawn (fn () (sleep {SLEEP_MS}) 2))))
          (+ (async/await a) (async/await b)))
        "#
    );
    let start = Instant::now();
    let result = interp
        .eval_str_via_runtime(&src)
        .expect("runtime eval of overlapping sleeps");
    let elapsed = start.elapsed();

    assert_eq!(result, Value::int(3), "both spawned tasks returned");
    // Serial execution would take >= 2 * SLEEP_MS (~400ms). Overlapping completes
    // in ~SLEEP_MS (~200ms). The 350ms ceiling cleanly separates the two.
    assert!(
        elapsed < Duration::from_millis(350),
        "expected the two blocking sleeps to overlap (~{SLEEP_MS}ms); took {elapsed:?}"
    );
}

/// A single blocking sleep still completes (and takes at least its duration):
/// guards against the executor path returning early without actually waiting.
#[test]
fn single_blocking_sleep_completes_through_executor() {
    let interp = Interpreter::new();
    let start = Instant::now();
    let result = interp
        .eval_str_via_runtime(&format!(
            "(async/await (async/spawn (fn () (sleep {SLEEP_MS}) 42)))"
        ))
        .expect("runtime eval of a single spawned sleep");
    let elapsed = start.elapsed();

    assert_eq!(result, Value::int(42));
    assert!(
        elapsed >= Duration::from_millis(SLEEP_MS - 40),
        "the sleep must actually elapse on the worker; took only {elapsed:?}"
    );
}

/// A top-level (non-spawned) `sleep` on the runtime path also suspends onto a
/// worker and resumes correctly, returning nil.
#[test]
fn top_level_blocking_sleep_returns_nil() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str_via_runtime("(sleep 10)")
        .expect("runtime eval of a top-level sleep");
    assert_eq!(result, Value::nil());
}
