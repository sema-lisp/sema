//! Cooperative-callback coverage for key-projecting list operations.

use sema_core::Value;
use sema_eval::Interpreter;

fn eval(source: &str) -> Result<Value, sema_core::SemaError> {
    Interpreter::new().eval_str_compiled(source)
}

#[test]
fn group_by_callback_suspends_and_preserves_group_order() {
    let result = eval(
        r#"
        (let ((out (channel/new 8)))
          (let ((grouped
                  (list/group-by
                    (fn (item)
                      (channel/send out item)
                      (async/sleep 1)
                      (even? item))
                    (list 1 2 3 4))))
            (list
              (get grouped #f)
              (get grouped #t)
              (list
                (channel/recv out) (channel/recv out)
                (channel/recv out) (channel/recv out)))))
        "#,
    )
    .expect("list/group-by callback should run through the runtime ABI");

    assert_eq!(
        result,
        Value::list(vec![
            Value::list(vec![Value::int(1), Value::int(3)]),
            Value::list(vec![Value::int(2), Value::int(4)]),
            Value::list(vec![
                Value::int(1),
                Value::int(2),
                Value::int(3),
                Value::int(4),
            ]),
        ])
    );
}

#[test]
fn key_by_callback_suspends_and_keeps_the_last_item_for_a_key() {
    let result = eval(
        r#"
        (let ((out (channel/new 8)))
          (let ((keyed
                  (list/key-by
                    (fn (item)
                      (let ((id (:id item)))
                        (channel/send out id)
                        (async/sleep 1)
                        id))
                    (list
                      {:id 1 :name "first"}
                      {:id 1 :name "last"}
                      {:id 2 :name "other"}))))
            (list
              (:name (get keyed 1))
              (:name (get keyed 2))
              (list
                (channel/recv out)
                (channel/recv out)
                (channel/recv out)))))
        "#,
    )
    .expect("list/key-by callback should suspend cooperatively");

    assert_eq!(
        result,
        Value::list(vec![
            Value::string("last"),
            Value::string("other"),
            Value::list(vec![Value::int(1), Value::int(1), Value::int(2)]),
        ])
    );
}

#[test]
fn key_projectors_support_a_direct_runtime_native_callback() {
    let result = eval(
        r#"
        (let ((grouped (list/group-by async/resolved (list 1)))
              (keyed (list/key-by async/resolved (list 2))))
          (list
            (type grouped)
            (type keyed)
            (async/resolved? (first (keys grouped)))
            (async/resolved? (first (keys keyed)))))
        "#,
    )
    .expect("key projectors should structurally invoke runtime natives");

    assert_eq!(
        result,
        Value::list(vec![
            Value::keyword("map"),
            Value::keyword("map"),
            Value::bool(true),
            Value::bool(true),
        ])
    );
}

#[test]
fn key_projectors_preserve_nan_key_collision_semantics() {
    let result = eval(
        r#"
        (let ((grouped
                (list/group-by
                  (fn (item) (async/sleep 1) math/nan)
                  (list 1 2)))
              (keyed
                (list/key-by
                  (fn (item) (async/sleep 1) math/nan)
                  (list 1 2))))
          (list
            (count grouped)
            (first (vals grouped))
            (count keyed)
            (first (vals keyed))))
        "#,
    )
    .expect("NaN keys should follow the legacy explicit-insertion path");

    assert_eq!(
        result,
        Value::list(vec![
            Value::int(1),
            Value::list(vec![Value::int(2)]),
            Value::int(1),
            Value::int(2),
        ])
    );
}

#[test]
fn group_by_snapshots_mutable_array_inputs() {
    let result = eval(
        r#"
        (let ((items (mutable-array/new)))
          (mutable-array/push! items 1)
          (mutable-array/push! items 2)
          (let ((grouped
                  (list/group-by
                    (fn (item)
                      (mutable-array/push! items 99)
                      (async/sleep 1)
                      (even? item))
                    items)))
            (list
              (mutable-array/length items)
              (get grouped #f)
              (get grouped #t))))
        "#,
    )
    .expect("list/group-by should iterate over one mutable-array snapshot");

    assert_eq!(
        result,
        Value::list(vec![
            Value::int(4),
            Value::list(vec![Value::int(1)]),
            Value::list(vec![Value::int(2)]),
        ])
    );
}

#[test]
fn key_projection_callback_observes_context_and_preserves_item_identity() {
    let result = eval(
        r#"
        (context/with
          {:offset 10}
          (fn ()
            (let ((first (mutable-cell/new 1))
                  (second (mutable-cell/new 2)))
              (let ((keyed
                      (list/key-by
                        (fn (item)
                          (async/sleep 1)
                          (+ (mutable-cell/get item)
                             (context/get :offset)))
                        (list first second))))
                (list
                  (eq? (get keyed 11) first)
                  (eq? (get keyed 12) second))))))
        "#,
    )
    .expect("key projection should inherit context and retain exact items");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true)])
    );
}

#[test]
fn key_projection_callback_parks_while_a_sibling_runs() {
    let result = eval(
        r#"
        (let ((out (channel/new 4)))
          (let ((grouping
                  (async/spawn
                    (fn ()
                      (list/group-by
                        (fn (item)
                          (async/sleep 100)
                          (channel/send out "grouping")
                          item)
                        (list 1)))))
                (sibling
                  (async/spawn
                    (fn ()
                      (async/sleep 10)
                      (channel/send out "sibling")))))
            (let ((first (channel/recv out)))
              (async/await grouping)
              (async/await sibling)
              first)))
        "#,
    )
    .expect("a parked key projection should yield to sibling tasks");

    assert_eq!(result.as_str(), Some("sibling"));
}

#[test]
fn key_projection_failure_stops_before_later_items() {
    let result = eval(
        r#"
        (let ((seen 0))
          (try
            (list/group-by
              (fn (item)
                (set! seen (+ seen 1))
                (async/sleep 1)
                (if (= item 2) (error "stop") item))
              (list 1 2 3))
            (catch error nil))
          seen)
        "#,
    )
    .expect("key projection failure should be catchable");

    assert_eq!(result.as_int(), Some(2));
}

#[test]
fn cancelling_a_key_projection_stops_before_later_items() {
    let result = eval(
        r#"
        (let ((seen 0))
          (let ((pending
                  (async/spawn
                    (fn ()
                      (list/key-by
                        (fn (item)
                          (set! seen (+ seen 1))
                          (async/sleep 100)
                          item)
                        (list 1 2))))))
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
    .expect("cancelling a key projection should settle the task");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true), Value::int(1)])
    );
}
