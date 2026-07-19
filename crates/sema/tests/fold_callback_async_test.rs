//! Cooperative-callback coverage for right and typed-array folds.

use sema_core::Value;
use sema_eval::Interpreter;

fn eval(source: &str) -> Result<Value, sema_core::SemaError> {
    Interpreter::new().eval_str_compiled(source)
}

#[test]
fn foldr_callback_suspends_in_right_to_left_order() {
    let result = eval(
        r#"
        (let ((out (channel/new 4)))
          (let ((folded
                  (foldr
                    (fn (item acc)
                      (channel/send out item)
                      (async/sleep 1)
                      (cons item acc))
                    (list)
                    (list 1 2 3))))
            (list
              folded
              (channel/recv out)
              (channel/recv out)
              (channel/recv out))))
        "#,
    )
    .expect("foldr callback should run through the runtime ABI");

    assert_eq!(
        result,
        Value::list(vec![
            Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
            Value::int(3),
            Value::int(2),
            Value::int(1),
        ])
    );
}

#[test]
fn typed_folds_suspend_and_support_a_direct_runtime_native_callback() {
    let result = eval(
        r#"
        (let ((out (channel/new 8)))
          (let ((sent
                  (i64-array/fold channel/send out (i64-array 42))))
            (let ((float-sum
                    (f64-array/fold
                      (fn (acc item)
                        (channel/send out item)
                        (async/sleep 1)
                        (+ acc item))
                      0.0
                      (f64-array/from-list (list 1.5 2.5))))
                  (int-sum
                    (i64-array/fold
                      (fn (acc item)
                        (channel/send out item)
                        (async/sleep 1)
                        (+ acc item))
                      0
                      (i64-array 1 2 3))))
              (list
                sent
                float-sum
                int-sum
                (list
                  (channel/recv out) (channel/recv out)
                  (channel/recv out) (channel/recv out)
                  (channel/recv out) (channel/recv out))))))
        "#,
    )
    .expect("typed folds should structurally invoke every callback");

    assert_eq!(
        result,
        Value::list(vec![
            Value::nil(),
            Value::float(4.0),
            Value::int(6),
            Value::list(vec![
                Value::int(42),
                Value::float(1.5),
                Value::float(2.5),
                Value::int(1),
                Value::int(2),
                Value::int(3),
            ]),
        ])
    );
}

#[test]
fn shared_fold_driver_preserves_foldl_reduce_and_mutable_array_snapshots() {
    let result = eval(
        r#"
        (let ((out (channel/new 8))
              (items (mutable-array/new)))
          (mutable-array/push! items 1)
          (mutable-array/push! items 2)
          (let ((left
                  (foldl
                    (fn (acc item)
                      (channel/send out item)
                      (mutable-array/push! items 99)
                      (async/sleep 1)
                      (+ acc item))
                    0
                    items))
                (reduced
                  (reduce
                    (fn (acc item)
                      (channel/send out item)
                      (async/sleep 1)
                      (+ acc item))
                    (list 1 2 3))))
            (list
              left
              reduced
              (mutable-array/length items)
              (channel/recv out) (channel/recv out)
              (channel/recv out) (channel/recv out))))
        "#,
    )
    .expect("the shared fold driver should preserve forward-fold semantics");

    assert_eq!(
        result,
        Value::list(vec![
            Value::int(3),
            Value::int(6),
            Value::int(4),
            Value::int(1),
            Value::int(2),
            Value::int(2),
            Value::int(3),
        ])
    );
}

#[test]
fn fold_callback_observes_the_callers_task_context() {
    let result = eval(
        r#"
        (context/with
          {:increment 10}
          (fn ()
            (i64-array/fold
              (fn (acc item)
                (async/sleep 1)
                (+ acc item (context/get :increment)))
              0
              (i64-array 1 2))))
        "#,
    )
    .expect("typed fold callback should inherit task context");

    assert_eq!(result.as_int(), Some(23));
}

#[test]
fn fold_callback_parks_while_a_sibling_runs() {
    let result = eval(
        r#"
        (let ((out (channel/new 4)))
          (let ((fold
                  (async/spawn
                    (fn ()
                      (f64-array/fold
                        (fn (acc item)
                          (async/sleep 100)
                          (channel/send out "fold")
                          (+ acc item))
                        0.0
                        (f64-array/from-list (list 1.0))))))
                (sibling
                  (async/spawn
                    (fn ()
                      (async/sleep 10)
                      (channel/send out "sibling")))))
            (let ((first (channel/recv out)))
              (async/await fold)
              (async/await sibling)
              first)))
        "#,
    )
    .expect("a parked fold callback should yield to sibling tasks");

    assert_eq!(result.as_str(), Some("sibling"));
}

#[test]
fn fold_callback_failure_stops_before_later_items() {
    let result = eval(
        r#"
        (let ((seen 0))
          (try
            (foldr
              (fn (item acc)
                (set! seen (+ seen 1))
                (async/sleep 1)
                (if (= item 2)
                  (error "stop")
                  (+ item acc)))
              0
              (list 1 2 3))
            (catch error nil))
          seen)
        "#,
    )
    .expect("fold callback failure should be catchable");

    assert_eq!(result.as_int(), Some(2));
}

#[test]
fn cancelling_a_fold_callback_stops_before_later_items() {
    let result = eval(
        r#"
        (let ((seen 0))
          (let ((pending
                  (async/spawn
                    (fn ()
                      (i64-array/fold
                        (fn (acc item)
                          (set! seen (+ seen 1))
                          (async/sleep 100)
                          (+ acc item))
                        0
                        (i64-array 1 2))))))
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
    .expect("cancelling a fold callback should settle the task");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true), Value::int(1)])
    );
}
