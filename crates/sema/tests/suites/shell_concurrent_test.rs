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
//! These tests use local subprocesses and no network.
//!
//! - Overlap: each of five Python children publishes a ready marker and waits
//!   for a shared release file. The controller releases the children only after
//!   all five markers exist, proving overlap without a wall-clock speed limit.
//!   Each result's stdout/exit is correct and in input order.
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

struct ShellBarrierDir(std::path::PathBuf);

impl Drop for ShellBarrierDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// All five children must be waiting at the barrier before any can finish.
#[test]
#[serial]
fn shell_concurrent_overlap() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let dir = ShellBarrierDir(
        std::env::temp_dir().join(format!("sema-shell-overlap-{}-{stamp}", std::process::id())),
    );
    std::fs::create_dir(&dir.0).expect("barrier directory");
    let root = dir.0.clone();
    let controller = std::thread::spawn(move || {
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        let all_ready = loop {
            if (0..5).all(|i| root.join(format!("ready-{i}")).exists()) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        // Release even on failure so the interpreter can reap its children.
        std::fs::write(root.join("release"), b"").expect("release children");
        all_ready
    });
    let script = r#"
import pathlib, sys, time
root = pathlib.Path(sys.argv[1])
(root / ("ready-" + sys.argv[2])).touch()
deadline = time.monotonic() + 60
while not (root / "release").exists():
    if time.monotonic() >= deadline:
        raise RuntimeError("controller did not release the shell barrier")
    time.sleep(0.01)
sys.stdout.buffer.write(b"done\n")
"#;
    let script = serde_json::to_string(script).expect("encode child script");
    let path = serde_json::to_string(dir.0.to_str().expect("UTF-8 fixture path"))
        .expect("encode fixture path");
    let program = format!(
        r#"
        (async/all
          (map (fn (i)
                 (async/spawn
                   (fn () (shell "python3" "-c" {script} {path} (number->string i)))))
               (list 0 1 2 3 4)))
    "#
    );
    let interp = Interpreter::new();
    let result = interp.eval_str_compiled(&program);
    let all_ready = controller.join().expect("barrier controller joined");
    let result = result.expect("concurrent shell program evaluated");

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
    assert!(
        all_ready,
        "all five shell children must start before the controller releases them"
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
    // The single-string form runs through the platform shell (`sh -c` /
    // `cmd /C`), so the variable reference must use that shell's expansion
    // syntax — `cmd` leaves `$VAR` literal.
    let var_ref = if cfg!(windows) {
        "%SEMA_ASYNC_FOO%"
    } else {
        "$SEMA_ASYNC_FOO"
    };
    let program = format!(
        r#"
        (first
          (async/all
            (list
              (async/spawn
                (fn () (:stdout (shell "echo {var_ref}"
                                       {{:env {{"SEMA_ASYNC_FOO" "async-bar"}}}})))))))
    "#
    );
    let result = interp
        .eval_str_compiled(&program)
        .expect("async shell with :env evaluated");
    assert_eq!(
        result.as_str().map(str::trim),
        Some("async-bar"),
        "async shell must honor the :env options map"
    );
}
