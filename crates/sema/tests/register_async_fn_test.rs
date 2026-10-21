//! Acceptance checks for `Interpreter::register_async_fn`
//! (`docs/plans/2026-10-21-async-native-registration.md`): a host registers an
//! async function through the public crate only, its future runs on Sema's I/O
//! pool while the calling task suspends, and every settlement path (success,
//! host error, conversion error, cancellation, worker panic, interpreter drop)
//! leaves no runtime state behind.

#![cfg(not(target_arch = "wasm32"))]

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use sema::{Interpreter, SemaError, Value};
use tokio::sync::oneshot;

const WAIT: Duration = Duration::from_secs(5);

fn expect_one_string(args: &[Value]) -> sema::Result<String> {
    if args.len() != 1 {
        return Err(SemaError::eval("expected one string argument"));
    }
    args[0]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))
}

fn assert_runtime_quiescent(interp: &Interpreter) {
    assert_eq!(interp.runtime_live_task_count(), 0, "live tasks remain");
    assert_eq!(
        interp.runtime_resource_gate_count(),
        0,
        "resource gates remain"
    );
}

/// Sends on `tx` when dropped, so a test can observe the host future being
/// torn down on the executor.
struct DropSignal(mpsc::Sender<&'static str>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        let _ = self.0.send("dropped");
    }
}

/// Register `name` as a host function whose future never completes on its own;
/// the returned receiver fires once the future is dropped.
fn register_forever(interp: &Interpreter, name: &str) -> mpsc::Receiver<&'static str> {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    interp.register_async_fn(
        name,
        |_args| Ok(()),
        move |_: ()| {
            let guard = DropSignal(tx.lock().expect("drop signal lock").clone());
            async move {
                let _guard = guard;
                std::future::pending::<()>().await;
                Ok::<(), Infallible>(())
            }
        },
        |_| Ok(Value::nil()),
    );
    rx
}

#[test]
fn delayed_keyword_is_built_on_the_interpreter_thread() {
    let interp = Interpreter::new();
    interp.register_async_fn(
        "host/delayed-keyword",
        expect_one_string,
        |name: String| async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            // Pollute the worker thread's interner: the keyword must still be
            // built from the owned name on the interpreter thread.
            let _ = sema::intern("register-async-unrelated-name");
            let _ = sema::intern(&name);
            Ok::<String, Infallible>(name)
        },
        |name| Ok(Value::keyword(&name)),
    );

    let value = interp
        .eval_str("(host/delayed-keyword \"ready\")")
        .expect("async host function resolves");
    assert_eq!(value, Value::keyword("ready"));
    assert_eq!(value.as_keyword().as_deref(), Some("ready"));

    // Synchronous registration keeps working beside it.
    interp.register_fn("host/square", |args| {
        let n = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
        Ok(Value::int(n * n))
    });
    assert_eq!(
        interp
            .eval_str("(host/square 7)")
            .expect("sync host function"),
        Value::int(49)
    );
    assert_runtime_quiescent(&interp);
}

#[test]
fn pending_host_future_lets_another_sema_task_run() {
    let interp = Interpreter::new();
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let release_tx = Arc::new(Mutex::new(Some(release_tx)));
    let release_rx = Arc::new(Mutex::new(Some(release_rx)));

    let waiter = Arc::clone(&release_rx);
    interp.register_async_fn(
        "host/wait",
        |_args| Ok(()),
        move |_: ()| {
            let waiter = Arc::clone(&waiter);
            async move {
                let rx = waiter
                    .lock()
                    .expect("release receiver lock")
                    .take()
                    .expect("host/wait is called once");
                tokio::time::timeout(WAIT, rx)
                    .await
                    .map_err(|_| std::io::Error::other("host/wait: no other task released it"))?
                    .map_err(|_| std::io::Error::other("release sender dropped"))?;
                Ok::<(), std::io::Error>(())
            }
        },
        |_| Ok(Value::nil()),
    );
    let marks = Arc::clone(&order);
    interp.register_fn("host/mark", move |args| {
        let label = expect_one_string(args)?;
        marks.lock().expect("order lock").push(label);
        Ok(Value::nil())
    });
    let releaser = Arc::clone(&release_tx);
    interp.register_fn("host/release", move |_| {
        if let Some(tx) = releaser.lock().expect("release sender lock").take() {
            let _ = tx.send(());
        }
        Ok(Value::nil())
    });

    let value = interp
        .eval_str(
            r#"
            (define p (async/spawn (fn () (host/mark "spawned-ran") (host/release) :done)))
            (host/wait)
            (host/mark "main-resumed")
            (async/await p)
            "#,
        )
        .expect("suspended root lets the spawned task run");
    assert_eq!(value, Value::keyword("done"));
    assert_eq!(
        *order.lock().expect("order lock"),
        vec!["spawned-ran".to_string(), "main-resumed".to_string()]
    );
    assert_runtime_quiescent(&interp);
}

#[test]
fn concurrent_calls_overlap_on_the_executor() {
    const CALLS: usize = 4;
    let interp = Interpreter::new();
    let barrier = Arc::new(tokio::sync::Barrier::new(CALLS));
    let runs = Arc::new(AtomicUsize::new(0));
    let run_counter = Arc::clone(&runs);
    interp.register_async_fn(
        "host/gate",
        |args| {
            args[0]
                .as_int()
                .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))
        },
        move |index: i64| {
            let barrier = Arc::clone(&barrier);
            run_counter.fetch_add(1, Ordering::SeqCst);
            async move {
                // Every call must be in flight at once for the barrier to open.
                tokio::time::timeout(WAIT, barrier.wait())
                    .await
                    .map_err(|_| std::io::Error::other("calls did not overlap"))?;
                Ok::<i64, std::io::Error>(index)
            }
        },
        |index| Ok(Value::int(index)),
    );

    let value = interp
        .eval_str(
            "(async/all (list (async (host/gate 1)) (async (host/gate 2))
                              (async (host/gate 3)) (async (host/gate 4))))",
        )
        .expect("four overlapping host calls");
    assert_eq!(value.to_string(), "(1 2 3 4)", "results keep input order");
    assert_eq!(runs.load(Ordering::SeqCst), CALLS);
    assert_runtime_quiescent(&interp);
}

#[test]
fn prepare_error_submits_no_job() {
    let interp = Interpreter::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let run_counter = Arc::clone(&runs);
    interp.register_async_fn(
        "host/echo",
        expect_one_string,
        move |text: String| {
            run_counter.fetch_add(1, Ordering::SeqCst);
            async move { Ok::<String, Infallible>(text) }
        },
        |text| Ok(Value::string(&text)),
    );

    let error = interp
        .eval_str("(host/echo 42)")
        .expect_err("prepare rejects a non-string");
    assert!(
        matches!(error.inner(), SemaError::Type { .. }),
        "prepare's own error is raised unchanged: {error}"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 0, "no job was submitted");
    assert_runtime_quiescent(&interp);
}

#[test]
fn finish_error_is_raised_unchanged() {
    let interp = Interpreter::new();
    interp.register_async_fn(
        "host/reject-on-finish",
        |_args| Ok(()),
        |_: ()| async { Ok::<(), Infallible>(()) },
        |_| Err(SemaError::type_error("keyword", "nil")),
    );
    let error = interp
        .eval_str("(host/reject-on-finish)")
        .expect_err("finish error propagates");
    assert!(
        matches!(error.inner(), SemaError::Type { .. }),
        "unexpected error: {error}"
    );
    assert_runtime_quiescent(&interp);
}

#[derive(Debug)]
struct HostFailure;

impl std::fmt::Display for HostFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("upstream refused")
    }
}

impl std::error::Error for HostFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&CAUSE)
    }
}

#[derive(Debug)]
struct Cause;

impl std::fmt::Display for Cause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("connection reset")
    }
}

impl std::error::Error for Cause {}

static CAUSE: Cause = Cause;

#[test]
fn host_error_is_raised_as_io_error_with_the_function_name() {
    let interp = Interpreter::new();
    interp.register_async_fn(
        "host/fail",
        |_args| Ok(()),
        |_: ()| async { Err::<(), HostFailure>(HostFailure) },
        |_| Ok(Value::nil()),
    );

    let error = interp
        .eval_str("(host/fail)")
        .expect_err("host error propagates");
    assert!(
        matches!(error.inner(), SemaError::Io(message) if message == "host/fail: upstream refused: connection reset"),
        "unexpected error: {error}"
    );

    // Catchable from Sema with the :io classification.
    let caught = interp
        .eval_str("(try (host/fail) (catch e (list (:type e) (:message e))))")
        .expect("try/catch observes the host error");
    assert_eq!(
        caught.to_string(),
        "(:io \"host/fail: upstream refused: connection reset\")"
    );
    assert_runtime_quiescent(&interp);
}

#[test]
fn macro_transformer_cannot_call_an_async_native() {
    let interp = Interpreter::new();
    interp.register_async_fn(
        "host/delayed-keyword",
        expect_one_string,
        |name: String| async move { Ok::<String, Infallible>(name) },
        |name| Ok(Value::keyword(&name)),
    );
    interp
        .eval_str("(defmacro expand-async () (host/delayed-keyword \"x\"))")
        .expect("register transformer");

    let error = interp
        .eval_str("(expand-async)")
        .expect_err("expansion cannot suspend");
    assert!(
        error
            .to_string()
            .contains("host/delayed-keyword: async host function cannot run here"),
        "unexpected error: {error}"
    );
    assert_runtime_quiescent(&interp);
}

#[test]
fn with_timeout_cancels_and_drops_the_host_future() {
    let interp = Interpreter::new();
    let dropped = register_forever(&interp, "host/forever");

    let error = interp
        .eval_str("(async/with-timeout 20 (fn () (host/forever)))")
        .expect_err("deadline wins");
    let message = error.to_string();
    assert!(
        !message.contains("I/O error"),
        "cancellation must not read as a host error: {message}"
    );
    let caught = interp
        .eval_str("(try (async/with-timeout 20 (fn () (host/forever))) (catch e (:type e)))")
        .expect("timeout condition is catchable");
    assert_eq!(
        caught,
        Value::keyword("timeout"),
        "timeout classification, not a host error"
    );
    assert_eq!(
        dropped.recv_timeout(WAIT),
        Ok("dropped"),
        "cancel signal drops the host future"
    );
    assert_runtime_quiescent(&interp);
}

#[test]
fn async_cancel_settles_the_task_and_drops_the_host_future() {
    let interp = Interpreter::new();
    let dropped = register_forever(&interp, "host/forever");

    let value = interp
        .eval_str(
            r#"
            (define p (async (host/forever)))
            (async/sleep 1)
            (async/cancel p)
            (try (async/await p) (catch e (:type e)))
            "#,
        )
        .expect("awaiting a cancelled promise is catchable");
    assert_eq!(value, Value::keyword("cancelled"));
    assert_eq!(dropped.recv_timeout(WAIT), Ok("dropped"));
    assert_runtime_quiescent(&interp);
}

#[cfg(panic = "unwind")]
#[test]
fn host_future_panic_is_an_error_not_an_abort() {
    let interp = Interpreter::new();
    interp.register_async_fn(
        "host/panic",
        |_args| Ok(()),
        |_: ()| async {
            if std::hint::black_box(true) {
                panic!("host future exploded");
            }
            Ok::<(), Infallible>(())
        },
        |_| Ok(Value::nil()),
    );

    let error = interp
        .eval_str("(host/panic)")
        .expect_err("panic surfaces as an error");
    assert!(
        error.to_string().contains("host/panic: worker panicked"),
        "unexpected error: {error}"
    );
    assert_runtime_quiescent(&interp);
}

#[test]
fn interpreter_drop_cancels_a_pending_cooperative_future() {
    let dropped;
    let elapsed;
    let weak_bindings = {
        let interp = Interpreter::new();
        dropped = register_forever(&interp, "host/forever");
        interp
            .eval_str("(define p (async (host/forever))) (async/sleep 1)")
            .expect("spawned task parks on the host future");
        let weak = std::rc::Rc::downgrade(&interp.global_env().bindings);
        let start = Instant::now();
        drop(interp);
        elapsed = start.elapsed();
        weak
    };
    assert_eq!(dropped.recv_timeout(WAIT), Ok("dropped"));
    assert!(
        elapsed < Duration::from_secs(2),
        "cooperative future is torn down before the shutdown deadline: {elapsed:?}"
    );
    assert_eq!(
        weak_bindings.strong_count(),
        0,
        "no adapter capture keeps the interpreter graph alive"
    );
}

#[test]
fn interpreter_drop_returns_within_deadline_for_a_blocking_future() {
    let interp = Interpreter::new();
    let finished = Arc::new(AtomicUsize::new(0));
    let done = Arc::clone(&finished);
    interp.register_async_fn(
        "host/block",
        |_args| Ok(()),
        move |_: ()| {
            let done = Arc::clone(&done);
            async move {
                // Not cancel-safe: blocks the pool worker and ignores drop.
                std::thread::sleep(Duration::from_secs(3));
                done.fetch_add(1, Ordering::SeqCst);
                Ok::<(), Infallible>(())
            }
        },
        |_| Ok(Value::nil()),
    );
    interp
        .eval_str("(define p (async (host/block))) (async/sleep 1)")
        .expect("spawned task parks on the host future");

    let start = Instant::now();
    drop(interp);
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "drop does not join a non-cooperative future: {elapsed:?}"
    );
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "future was still blocked at drop"
    );
    std::thread::sleep(
        Duration::from_secs(3) - elapsed.min(Duration::from_secs(3)) + Duration::from_millis(200),
    );
    assert_eq!(
        finished.load(Ordering::SeqCst),
        1,
        "detached job completes on the pool without a runtime slot"
    );
}

#[test]
fn completed_calls_release_the_interpreter_graph() {
    let weak_bindings = {
        let interp = Interpreter::new();
        interp.register_async_fn(
            "host/echo",
            expect_one_string,
            |text: String| async move { Ok::<String, Infallible>(text) },
            |text| Ok(Value::string(&text)),
        );
        interp
            .eval_str("(define kept (host/echo \"kept-until-drop\"))")
            .expect("completed call");
        assert_runtime_quiescent(&interp);
        std::rc::Rc::downgrade(&interp.global_env().bindings)
    };
    assert_eq!(weak_bindings.strong_count(), 0);
}
