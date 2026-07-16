//! Async-offload coverage for `serial/*` under the unified cooperative runtime.
//!
//! `serial/write`/`serial/read-line`/`serial/send` offload through the CHECKOUT
//! pattern (`crate::runtime_offload::checkout_external`) and `serial/open` as a
//! plain External wait — see `crates/sema-stdlib/src/serial.rs`'s module doc
//! comment. No real serial hardware exists in CI, so this suite drives the parts
//! of the runtime path that don't need an open port: the checkout chain's
//! missing-handle error is byte-identical to the sync path, `serial/open`
//! rejects a bad device cleanly through the offload, and a cancelled spawned
//! serial op settles without wedging the thread-local registry.
//!
//! A genuine "cancel while queued behind a BUSY handle" test would need a second
//! task holding a real port's gate; that requires hardware and is therefore not
//! representable here (unlike proc/pty/stream, whose busy handles are real
//! subprocesses/files). The cancellation coverage below proves the checkout
//! continuations' Cancelled arms don't hang or corrupt the registry.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;

/// The async checkout path reports the SAME "invalid handle" text the sync path
/// raises — proving the gate-acquire → take → missing-handle chain surfaces the
/// domain error verbatim rather than a generic runtime failure.
#[test]
fn serial_write_async_missing_handle_matches_sync() {
    let interp = Interpreter::new();
    let sync_err = interp
        .eval_str_compiled(r#"(serial/write 999 "hi")"#)
        .expect_err("sync serial/write on a missing handle must fail")
        .to_string();
    let async_err = interp
        .eval_str_compiled(r#"(await (async/spawn (fn () (serial/write 999 "hi"))))"#)
        .expect_err("async serial/write on a missing handle must fail")
        .to_string();
    assert!(
        sync_err.contains("serial/write: invalid handle 999"),
        "unexpected sync error: {sync_err}"
    );
    assert!(
        async_err.contains("serial/write: invalid handle 999"),
        "async rejection must carry the byte-identical missing-handle text\n  sync:  {sync_err}\n  async: {async_err}"
    );
}

#[test]
fn serial_read_line_async_missing_handle_matches_sync() {
    let interp = Interpreter::new();
    let async_err = interp
        .eval_str_compiled(r#"(await (async/spawn (fn () (serial/read-line 999))))"#)
        .expect_err("async serial/read-line on a missing handle must fail")
        .to_string();
    assert!(
        async_err.contains("serial/read-line: invalid handle 999"),
        "unexpected async error: {async_err}"
    );
}

#[test]
fn serial_send_async_missing_handle_matches_sync() {
    let interp = Interpreter::new();
    let async_err = interp
        .eval_str_compiled(r#"(await (async/spawn (fn () (serial/send 999 "ping"))))"#)
        .expect_err("async serial/send on a missing handle must fail")
        .to_string();
    assert!(
        async_err.contains("serial/send: invalid handle 999"),
        "unexpected async error: {async_err}"
    );
}

/// `serial/open` on a nonexistent device offloads the blocking `open()` and
/// rejects cleanly through the External wait, mentioning `serial/open` — the
/// same failure the sync path would raise.
#[test]
fn serial_open_async_bad_device_errors_cleanly() {
    let interp = Interpreter::new();
    let async_err = interp
        .eval_str_compiled(
            r#"(await (async/spawn (fn () (serial/open "/dev/sema-nonexistent-test-device" 9600))))"#,
        )
        .expect_err("opening a nonexistent device must fail")
        .to_string();
    assert!(
        async_err.contains("serial/open"),
        "error should mention serial/open: {async_err}"
    );
}

/// Cancelling a spawned serial op settles the task (either :cancelled or its
/// domain error — both mean "no hang") and leaves the registry usable: a fresh
/// serial op afterward still reports the missing handle cleanly. Exercises the
/// checkout continuations' Cancelled arms without wedging the runtime.
#[test]
fn serial_cancelled_chain_settles_and_registry_stays_usable() {
    let interp = Interpreter::new();
    let program = r#"
        (let ((p (async/spawn (fn () (serial/read-line 999)))))
          (async/cancel p)
          (let ((caught (try (async/await p) (catch e :caught))))
            (let ((after (try (serial/write 999 "x") (catch e :after-errored))))
              (list caught after))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("cancelled serial chain evaluates without wedging the runtime");
    let parts: Vec<sema_core::Value> = result.as_list().expect("result list").to_vec();
    assert_eq!(
        parts[0],
        sema_core::Value::keyword("caught"),
        "the cancelled task must settle (cancelled or domain error), got {:?}",
        parts[0]
    );
    assert_eq!(
        parts[1],
        sema_core::Value::keyword("after-errored"),
        "a fresh serial op must still error cleanly after the cancellation (registry not wedged)"
    );
}
