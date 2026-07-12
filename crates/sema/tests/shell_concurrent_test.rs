//! Acceptance gate for concurrent subprocess execution (`shell` overlapping
//! under `async/spawn`).
//!
//! `shell` funnels through the builtin in `crates/sema-stdlib/src/system.rs`. At
//! top level it blocks on `std::process::Command::output()` (synchronous,
//! unchanged). Inside an `async/spawn`'d task it offloads the subprocess onto the
//! process-wide multi-thread runtime (`STDLIB_SHARED_RT`, shared with the
//! `http/*` slice) and yields `AwaitIo`, so several children overlap on the
//! single VM thread.
//!
//! These tests are pure local subprocess (no network) and fully deterministic:
//! `sh -c "sleep 0.5; echo done"` is the unit of overlap.
//!
//! - Overlap: five 0.5 s shells via `async/all`+`async/spawn`+`map` complete in
//!   ~0.5-1.0 s (overlapped), decisively below the ~2.5 s serial floor, and each
//!   result's stdout/exit is correct & in input order.
//! - Non-zero exit: a concurrent `sh -c "exit 3"` returns exit-code 3 in its
//!   result map (not an error / hang).
//! - Spawn error: a concurrent shell of a nonexistent program fails that task
//!   cleanly without hanging the scheduler.
//! - Sync path unchanged: a plain top-level `shell` returns the identical value
//!   shape (stdout "hi", exit 0).

#![cfg(not(target_arch = "wasm32"))]

use std::time::Instant;

use sema_core::Value;
use sema_eval::Interpreter;
use serial_test::serial;

/// Five 0.5 s shells run as five tasks via `async/all`+`async/spawn`+`map`.
/// Overlap means ~0.5-1.0 s, not the ~2.5 s serial floor. Each result's stdout
/// and exit code must be correct and in input order.
#[test]
#[serial]
fn shell_concurrent_overlap() {
    let interp = Interpreter::new();
    let program = r#"
        (async/all
          (map (fn (i)
                 (async/spawn
                   (fn () (shell "sh" "-c" "sleep 0.5; echo done"))))
               (list 0 1 2 3 4)))
    "#;

    let t0 = Instant::now();
    let result = interp
        .eval_str_compiled(program)
        .expect("concurrent shell program evaluated");
    let elapsed_ms = t0.elapsed().as_millis();

    // Correctness: five result maps, each {:stdout "done\n" :stderr "" :exit-code 0}.
    let one = || {
        let mut m = std::collections::BTreeMap::new();
        m.insert(Value::keyword("stdout"), Value::string("done\n"));
        m.insert(Value::keyword("stderr"), Value::string(""));
        m.insert(Value::keyword("exit-code"), Value::int(0));
        Value::map(m)
    };
    let expected = Value::list((0..5).map(|_| one()).collect());
    assert_eq!(
        result, expected,
        "expected five correct shell results in input order"
    );

    // Overlap (timing): serial floor is ~2500 ms; overlapping ~500-1000 ms.
    eprintln!("shell_concurrent_overlap: wall-clock {elapsed_ms} ms (serial floor ~2500 ms)");
    assert!(
        elapsed_ms < 2000,
        "expected overlapped wall-clock < 2000 ms (serial floor ~2500 ms), got {elapsed_ms} ms"
    );
}

/// A concurrent `sh -c "exit 3"` must report exit-code 3 in its result map —
/// not surface as an error and not hang the scheduler.
#[test]
#[serial]
fn shell_concurrent_nonzero_exit() {
    let interp = Interpreter::new();
    let program = r#"
        (first
          (async/all
            (list
              (async/spawn (fn () (:exit-code (shell "sh" "-c" "exit 3")))))))
    "#;

    let result = interp
        .eval_str_compiled(program)
        .expect("concurrent nonzero-exit shell program evaluated");
    assert_eq!(
        result,
        Value::int(3),
        "concurrent non-zero exit must propagate exit-code 3"
    );
}

/// A concurrent direct-exec shell of a nonexistent program must fail that task
/// cleanly and surface the error through `async/all` — without hanging the
/// scheduler. The direct (multi-arg) form runs the program directly (no `sh -c`
/// wrapper), so a missing binary is a genuine spawn error, exactly as the sync
/// path's `std::process::Command::output()` would return `Err`.
#[test]
#[serial]
fn shell_concurrent_spawn_error() {
    let interp = Interpreter::new();
    let program = r#"
        (async/all
          (list
            (async/spawn (fn () (shell "this-program-does-not-exist-xyz123" "arg")))))
    "#;

    let t0 = Instant::now();
    let result = interp.eval_str_compiled(program);
    let elapsed_ms = t0.elapsed().as_millis();

    assert!(
        result.is_err(),
        "expected the nonexistent-program shell to fail the task, got {result:?}"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("shell:"),
        "expected a shell spawn error, got: {msg}"
    );
    assert!(
        elapsed_ms < 5000,
        "spawn-error path should fail fast, not hang; took {elapsed_ms} ms"
    );
}

/// The synchronous (top-level, non-async) path must be untouched: a plain
/// `shell` returns the identical value shape (stdout "hi\n", stderr "", exit 0).
#[test]
#[serial]
fn shell_sync_path_unchanged() {
    let interp = Interpreter::new();
    let program = r#"(shell "sh" "-c" "echo hi")"#;

    let result = interp
        .eval_str_compiled(program)
        .expect("sync shell program evaluated");

    let mut expected = std::collections::BTreeMap::new();
    expected.insert(Value::keyword("stdout"), Value::string("hi\n"));
    expected.insert(Value::keyword("stderr"), Value::string(""));
    expected.insert(Value::keyword("exit-code"), Value::int(0));
    assert_eq!(
        result,
        Value::map(expected),
        "sync path must return the identical value shape"
    );
}

/// The async (offloaded) path must honor the trailing `{:env ...}` options map,
/// not silently drop it — the injected var must reach the child spawned on the
/// I/O pool.
#[test]
#[serial]
fn shell_async_honors_env_option() {
    let interp = Interpreter::new();
    let program = r#"
        (first
          (async/all
            (list
              (async/spawn
                (fn () (:stdout (shell "echo $SEMA_ASYNC_FOO"
                                       {:env {"SEMA_ASYNC_FOO" "async-bar"}})))))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("async shell with :env evaluated");
    assert_eq!(
        result.as_str().map(str::trim),
        Some("async-bar"),
        "async shell must honor the :env options map"
    );
}
