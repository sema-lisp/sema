//! Acceptance checks for `Interpreter::register_async_fn_cx` (#179): one
//! closure that returns a value now or awaits a future, spawns tasks on Sema's
//! pool without a host runtime handle, and exposes a cancellation token tied
//! to the calling Sema task.

#![cfg(not(target_arch = "wasm32"))]

use std::convert::Infallible;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use sema::{Interpreter, SemaError, Value};

const WAIT: Duration = Duration::from_secs(5);

fn assert_quiescent(interp: &Interpreter) {
    assert_eq!(interp.runtime_live_task_count(), 0);
    assert_eq!(interp.runtime_resource_gate_count(), 0);
}

fn register_echo(interp: &Interpreter) {
    interp.register_async_fn_cx("host/echo", |cx, args| {
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_owned();
        if text.is_empty() {
            return Ok(Value::keyword("empty").into());
        }
        Ok(cx.await_future(
            move || async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
                Ok::<String, Infallible>(text.to_uppercase())
            },
            |text| Ok(Value::string(&text)),
        ))
    });
}

#[test]
fn ready_and_awaited_results() {
    let interp = Interpreter::new();
    register_echo(&interp);
    assert_eq!(
        interp.eval_str("(host/echo \"\")").unwrap(),
        Value::keyword("empty")
    );
    assert_eq!(
        interp.eval_str("(host/echo \"hi\")").unwrap(),
        Value::string("HI")
    );
    assert_quiescent(&interp);
}

#[test]
fn awaiting_suspends_only_the_calling_task() {
    let interp = Interpreter::new();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = Arc::new(Mutex::new(release_rx));
    interp.register_async_fn_cx("host/wait", move |cx, _args| {
        let rx = Arc::clone(&release_rx);
        Ok(cx.await_future(
            move || async move {
                tokio::task::spawn_blocking(move || rx.lock().unwrap().recv_timeout(WAIT))
                    .await
                    .map_err(std::io::Error::other)?
                    .map_err(|_| std::io::Error::other("never released"))?;
                Ok::<(), std::io::Error>(())
            },
            |_| Ok(Value::keyword("released")),
        ))
    });
    let release_tx = Mutex::new(Some(release_tx));
    interp.register_fn("host/release", move |_| {
        if let Some(tx) = release_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
        Ok(Value::nil())
    });
    let value = interp
        .eval_str("(define p (async (host/release))) (host/wait)")
        .expect("spawned task runs while the root is suspended");
    assert_eq!(value, Value::keyword("released"));
    assert_quiescent(&interp);
}

#[test]
fn spawn_runs_on_the_pool_without_a_host_runtime() {
    let interp = Interpreter::new();
    let (tx, rx) = mpsc::channel::<String>();
    let tx = Mutex::new(tx);
    interp.register_async_fn_cx("host/fire", move |cx, _args| {
        let tx = tx.lock().unwrap().clone();
        cx.spawn(move || async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let name = std::thread::current().name().unwrap_or("").to_owned();
            let _ = tx.send(name);
        });
        Ok(Value::nil().into())
    });
    interp.eval_str("(host/fire)").unwrap();
    let thread = rx.recv_timeout(WAIT).expect("spawned task ran");
    assert!(thread.starts_with("sema-io-"), "ran on {thread:?}");
    assert_quiescent(&interp);
}

#[test]
fn cancellation_token_fires_for_spawned_tasks_and_drops_the_future() {
    let interp = Interpreter::new();
    let (event_tx, event_rx) = mpsc::channel::<&'static str>();
    let event_tx = Mutex::new(event_tx);
    interp.register_async_fn_cx("host/forever", move |cx, _args| {
        let token = cx.cancellation_token();
        let spawned = event_tx.lock().unwrap().clone();
        cx.spawn(move || async move {
            token.cancelled().await;
            let _ = spawned.send("spawned-saw-cancel");
        });
        let dropped = event_tx.lock().unwrap().clone();
        Ok(cx.await_future(
            move || async move {
                struct Guard(mpsc::Sender<&'static str>);
                impl Drop for Guard {
                    fn drop(&mut self) {
                        let _ = self.0.send("future-dropped");
                    }
                }
                let _guard = Guard(dropped);
                std::future::pending::<()>().await;
                Ok::<(), Infallible>(())
            },
            |_| Ok(Value::nil()),
        ))
    });
    let caught = interp
        .eval_str("(try (async/with-timeout 20 (fn () (host/forever))) (catch e (:type e)))")
        .unwrap();
    assert_eq!(caught, Value::keyword("timeout"));
    let mut events = vec![
        event_rx.recv_timeout(WAIT).unwrap(),
        event_rx.recv_timeout(WAIT).unwrap(),
    ];
    events.sort();
    assert_eq!(events, vec!["future-dropped", "spawned-saw-cancel"]);
    assert_quiescent(&interp);
}

#[test]
fn host_error_is_an_io_error() {
    let interp = Interpreter::new();
    interp.register_async_fn_cx("host/fail", |cx, _args| {
        Ok(cx.await_future(
            || async { Err::<(), std::io::Error>(std::io::Error::other("refused")) },
            |_| Ok(Value::nil()),
        ))
    });
    let caught = interp
        .eval_str("(try (host/fail) (catch e (list (:type e) (:message e))))")
        .unwrap();
    assert_eq!(caught.to_string(), "(:io \"host/fail: refused\")");
    assert_quiescent(&interp);
}

#[test]
fn macro_expansion_accepts_ready_values_and_rejects_awaits() {
    let interp = Interpreter::new();
    register_echo(&interp);
    interp
        .eval_str("(defmacro ready-at-expand () (host/echo \"\"))")
        .unwrap();
    assert_eq!(
        interp.eval_str("(ready-at-expand)").unwrap(),
        Value::keyword("empty")
    );
    interp
        .eval_str("(defmacro await-at-expand () (host/echo \"x\"))")
        .unwrap();
    let error = interp.eval_str("(await-at-expand)").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("host/echo: async host function cannot run here"),
        "{error}"
    );
    assert_quiescent(&interp);
}

/// Registers `host/park`: requests a token, spawns a task that reports when
/// the token fires, and awaits a future that never resolves.
fn register_park(interp: &Interpreter) -> mpsc::Receiver<&'static str> {
    let (tx, rx) = mpsc::channel::<&'static str>();
    let tx = Mutex::new(tx);
    interp.register_async_fn_cx("host/park", move |cx, _args| {
        let token = cx.cancellation_token();
        let seen = tx.lock().unwrap().clone();
        cx.spawn(move || async move {
            token.cancelled().await;
            let _ = seen.send("token-fired");
        });
        Ok(cx.await_future(
            || async {
                std::future::pending::<()>().await;
                Ok::<(), Infallible>(())
            },
            |_| Ok(Value::nil()),
        ))
    });
    rx
}

#[test]
fn token_does_not_fire_when_the_call_completes() {
    let interp = Interpreter::new();
    let tokens: Arc<Mutex<Vec<sema::CancellationToken>>> = Arc::default();
    let kept = Arc::clone(&tokens);
    interp.register_async_fn_cx("host/quick", move |cx, _args| {
        kept.lock().unwrap().push(cx.cancellation_token());
        Ok(cx.await_future(|| async { Ok::<i64, Infallible>(1) }, |n| Ok(Value::int(n))))
    });
    assert_eq!(interp.eval_str("(host/quick)").unwrap(), Value::int(1));
    let tokens = tokens.lock().unwrap();
    assert_eq!(tokens.len(), 1);
    assert!(
        !tokens[0].is_cancelled(),
        "completion must not fire the token"
    );
    assert_quiescent(&interp);
}

#[test]
fn token_fires_on_async_cancel() {
    let interp = Interpreter::new();
    let events = register_park(&interp);
    let value = interp
        .eval_str(
            "(define p (async (host/park))) (async/sleep 1) (async/cancel p) \
             (try (async/await p) (catch e (:type e)))",
        )
        .unwrap();
    assert_eq!(value, Value::keyword("cancelled"));
    assert_eq!(events.recv_timeout(WAIT), Ok("token-fired"));
    assert_quiescent(&interp);
}

#[test]
fn token_fires_on_interpreter_drop() {
    let interp = Interpreter::new();
    let events = register_park(&interp);
    interp
        .eval_str("(define p (async (host/park))) (async/sleep 1)")
        .unwrap();
    drop(interp);
    assert_eq!(events.recv_timeout(WAIT), Ok("token-fired"));
}
