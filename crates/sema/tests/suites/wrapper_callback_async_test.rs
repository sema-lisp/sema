//! Cooperative-callback coverage for one-shot wrapper builtins.

use sema_core::Value;
use sema_eval::Interpreter;

fn eval(source: &str) -> Result<Value, sema_core::SemaError> {
    Interpreter::new().eval_str_compiled(source)
}

#[test]
fn tap_and_timing_thunks_can_suspend_and_preserve_results() {
    let result = eval(
        r#"
        (let ((seen (mutable-cell/new nil)))
          (let ((tapped
                  (tap
                    "original"
                    (fn (value)
                      (async/sleep 1)
                      (mutable-cell/set! seen value)
                      "ignored")))
                (timed
                  (time
                    (fn ()
                      (async/sleep 1)
                      42)))
                (elapsed
                  (time/ms
                    (fn ()
                      (async/sleep 1)
                      "ignored"))))
            (list tapped (mutable-cell/get seen) timed (type elapsed))))
        "#,
    )
    .expect("wrapper callbacks should run through the runtime ABI");

    assert_eq!(
        result,
        Value::list(vec![
            Value::string("original"),
            Value::string("original"),
            Value::int(42),
            Value::keyword("float"),
        ])
    );
}

#[test]
fn tap_supports_a_direct_runtime_native_callback() {
    let result = eval(
        r#"
        (let ((value (tap 7 async/resolved)))
          (list value (= value 7)))
        "#,
    )
    .expect("tap should structurally invoke a runtime-only native");

    assert_eq!(result, Value::list(vec![Value::int(7), Value::bool(true)]));
}

#[test]
fn wrapper_callbacks_observe_the_callers_task_context() {
    let result = eval(
        r#"
        (context/with
          {:increment 10}
          (fn ()
            (list
              (tap
                5
                (fn (value)
                  (async/sleep 1)
                  (+ value (context/get :increment))))
              (time
                (fn ()
                  (async/sleep 1)
                  (context/get :increment))))))
        "#,
    )
    .expect("wrapper callbacks should inherit task context");

    assert_eq!(result, Value::list(vec![Value::int(5), Value::int(10)]));
}

#[test]
fn a_timing_thunk_parks_while_a_sibling_runs() {
    let result = eval(
        r#"
        (let ((out (channel/new 4)))
          (let ((timed
                  (async/spawn
                    (fn ()
                      (time
                        (fn ()
                          (async/sleep 100)
                          (channel/send out "timed"))))))
                (sibling
                  (async/spawn
                    (fn ()
                      (async/sleep 10)
                      (channel/send out "sibling")))))
            (let ((first (channel/recv out)))
              (async/await timed)
              (async/await sibling)
              first)))
        "#,
    )
    .expect("a parked timing thunk should yield to sibling tasks");

    assert_eq!(result.as_str(), Some("sibling"));
}

#[test]
fn wrapper_callback_failures_are_catchable() {
    let result = eval(
        r#"
        (let ((seen 0))
          (let ((tap-error
                  (try
                    (tap
                      1
                      (fn (value)
                        (set! seen (+ seen value))
                        (async/sleep 1)
                        (error "tap stopped")))
                    (catch error "tap caught")))
                (time-error
                  (try
                    (time
                      (fn ()
                        (set! seen (+ seen 10))
                        (async/sleep 1)
                        (error "time stopped")))
                    (catch error "time caught"))))
            (list tap-error time-error seen)))
        "#,
    )
    .expect("wrapper callback failures should remain catchable");

    assert_eq!(
        result,
        Value::list(vec![
            Value::string("tap caught"),
            Value::string("time caught"),
            Value::int(11),
        ])
    );
}

#[test]
fn cancelling_a_wrapper_callback_settles_the_task() {
    let result = eval(
        r#"
        (let ((seen 0))
          (let ((pending
                  (async/spawn
                    (fn ()
                      (time/ms
                        (fn ()
                          (set! seen (+ seen 1))
                          (async/sleep 100)
                          (set! seen (+ seen 100))))))))
            (let ((canceller
                    (async/spawn
                      (fn ()
                        (async/sleep 10)
                        (async/cancel pending)))))
              (let ((requested (async/await canceller)))
                (try (async/await pending) (catch error nil))
                (list requested (async/cancelled? pending) seen)))))
        "#,
    )
    .expect("cancelling a wrapper callback should settle the task");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true), Value::int(1)])
    );
}
