//! Cooperative-callback coverage for predicate higher-order functions.

use sema_core::Value;
use sema_eval::Interpreter;

fn eval(source: &str) -> Result<Value, sema_core::SemaError> {
    Interpreter::new().eval_str_compiled(source)
}

#[test]
fn any_and_every_suspend_and_short_circuit_at_the_first_decisive_item() {
    let result = eval(
        r#"
        (let ((out (channel/new 8))
              (any-seen 0)
              (every-seen 0))
          (list
            (any?
              (fn (item)
                (set! any-seen (+ any-seen 1))
                (channel/send out item)
                (async/sleep 1)
                (= item 2))
              (list 1 2 3))
            any-seen
            (every?
              (fn (item)
                (set! every-seen (+ every-seen 1))
                (channel/send out item)
                (async/sleep 1)
                (< item 3))
              (list 1 2 3 4))
            every-seen
            (list
              (channel/recv out) (channel/recv out)
              (channel/recv out) (channel/recv out)
              (channel/recv out))))
        "#,
    )
    .expect("any/every predicates should run through the runtime ABI");

    assert_eq!(
        result,
        Value::list(vec![
            Value::bool(true),
            Value::int(2),
            Value::bool(false),
            Value::int(3),
            Value::list(vec![
                Value::int(1),
                Value::int(2),
                Value::int(1),
                Value::int(2),
                Value::int(3),
            ]),
        ])
    );
}

#[test]
fn partition_and_reject_suspend_and_preserve_full_scan_order() {
    let result = eval(
        r#"
        (let ((out (channel/new 8)))
          (let ((partitioned
                  (partition
                    (fn (item)
                      (channel/send out item)
                      (async/sleep 1)
                      (even? item))
                    (list 1 2 3 4)))
                (rejected
                  (list/reject
                    (fn (item)
                      (channel/send out item)
                      (async/sleep 1)
                      (even? item))
                    (list 1 2 3 4))))
            (list
              partitioned
              rejected
              (list
                (channel/recv out) (channel/recv out)
                (channel/recv out) (channel/recv out)
                (channel/recv out) (channel/recv out)
                (channel/recv out) (channel/recv out)))))
        "#,
    )
    .expect("full-scan predicates should suspend cooperatively");

    assert_eq!(
        result,
        Value::list(vec![
            Value::list(vec![
                Value::list(vec![Value::int(2), Value::int(4)]),
                Value::list(vec![Value::int(1), Value::int(3)]),
            ]),
            Value::list(vec![Value::int(1), Value::int(3)]),
            Value::list(vec![
                Value::int(1),
                Value::int(2),
                Value::int(3),
                Value::int(4),
                Value::int(1),
                Value::int(2),
                Value::int(3),
                Value::int(4),
            ]),
        ])
    );
}

#[test]
fn prefix_predicates_stop_calling_after_the_boundary() {
    let result = eval(
        r#"
        (let ((out (channel/new 16))
              (take-seen 0)
              (drop-seen 0)
              (list-take-seen 0)
              (list-drop-seen 0))
          (list
            (take-while
              (fn (item)
                (set! take-seen (+ take-seen 1))
                (channel/send out item)
                (async/sleep 1)
                (< item 3))
              (list 1 2 3 4))
            take-seen
            (drop-while
              (fn (item)
                (set! drop-seen (+ drop-seen 1))
                (channel/send out item)
                (async/sleep 1)
                (< item 3))
              (list 1 2 3 4))
            drop-seen
            (list/take-while
              (fn (item)
                (set! list-take-seen (+ list-take-seen 1))
                (channel/send out item)
                (async/sleep 1)
                (< item 3))
              (list 1 2 3 4))
            list-take-seen
            (list/drop-while
              (fn (item)
                (set! list-drop-seen (+ list-drop-seen 1))
                (channel/send out item)
                (async/sleep 1)
                (< item 3))
              (list 1 2 3 4))
            list-drop-seen
            (list
              (channel/recv out) (channel/recv out) (channel/recv out)
              (channel/recv out) (channel/recv out) (channel/recv out)
              (channel/recv out) (channel/recv out) (channel/recv out)
              (channel/recv out) (channel/recv out) (channel/recv out))))
        "#,
    )
    .expect("prefix predicates should suspend cooperatively");

    assert_eq!(
        result,
        Value::list(vec![
            Value::list(vec![Value::int(1), Value::int(2)]),
            Value::int(3),
            Value::list(vec![Value::int(3), Value::int(4)]),
            Value::int(3),
            Value::list(vec![Value::int(1), Value::int(2)]),
            Value::int(3),
            Value::list(vec![Value::int(3), Value::int(4)]),
            Value::int(3),
            Value::list(vec![
                Value::int(1),
                Value::int(2),
                Value::int(3),
                Value::int(1),
                Value::int(2),
                Value::int(3),
                Value::int(1),
                Value::int(2),
                Value::int(3),
                Value::int(1),
                Value::int(2),
                Value::int(3),
            ]),
        ])
    );
}

#[test]
fn list_find_returns_the_exact_item_seen_by_the_predicate() {
    let result = eval(
        r#"
        (context/with
          {:tag "task"}
          (fn ()
            (let ((seen (mutable-cell/new nil))
                  (first (mutable-cell/new 1))
                  (second (mutable-cell/new 2))
                  (third (mutable-cell/new 3)))
              (let ((found
                      (list/find
                        (fn (item)
                          (mutable-cell/set! seen item)
                          (async/sleep 1)
                          (and (= (context/get :tag) "task")
                               (= (mutable-cell/get item) 2)))
                        (list first second third))))
                (eq? found (mutable-cell/get seen))))))
        "#,
    )
    .expect("list/find should inherit context and preserve item identity");

    assert_eq!(result, Value::bool(true));
}

#[test]
fn list_sole_reports_the_second_match_without_visiting_later_items() {
    let result = eval(
        r#"
        (let ((out (channel/new 4))
              (seen 0))
          (try
            (list/sole
              (fn (item)
                (set! seen (+ seen 1))
                (channel/send out item)
                (async/sleep 1)
                (even? item))
              (list 2 4 6))
            (catch error nil))
          (list seen (channel/recv out) (channel/recv out)))
        "#,
    )
    .expect("list/sole's multiple-match error should be catchable");

    assert_eq!(
        result,
        Value::list(vec![Value::int(2), Value::int(2), Value::int(4)])
    );
}

#[test]
fn predicate_callback_parks_while_a_sibling_runs() {
    let result = eval(
        r#"
        (let ((out (channel/new 4)))
          (let ((predicate
                  (async/spawn
                    (fn ()
                      (any
                        (fn (item)
                          (async/sleep 100)
                          (channel/send out "predicate")
                          #t)
                        (list 1)))))
                (sibling
                  (async/spawn
                    (fn ()
                      (async/sleep 10)
                      (channel/send out "sibling")))))
            (let ((first (channel/recv out)))
              (async/await predicate)
              (async/await sibling)
              first)))
        "#,
    )
    .expect("a parked predicate callback should yield to sibling tasks");

    assert_eq!(result.as_str(), Some("sibling"));
}

#[test]
fn predicate_driver_supports_a_direct_runtime_native_callback() {
    let result = eval(
        r#"
        (let ((out (channel/new 4)))
          (let ((predicate
                  (async/spawn
                    (fn ()
                      (any async/sleep (list 100))
                      (channel/send out "predicate"))))
                (sibling
                  (async/spawn
                    (fn ()
                      (async/sleep 10)
                      (channel/send out "sibling")))))
            (let ((first (channel/recv out)))
              (async/await predicate)
              (async/await sibling)
              first)))
        "#,
    )
    .expect("a runtime-native predicate should be structurally invoked");

    assert_eq!(result.as_str(), Some("sibling"));
}

#[test]
fn cancelling_a_predicate_callback_stops_before_later_items() {
    let result = eval(
        r#"
        (let ((seen 0))
          (let ((pending
                  (async/spawn
                    (fn ()
                      (partition
                        (fn (item)
                          (set! seen (+ seen 1))
                          (async/sleep 100)
                          #t)
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
    .expect("cancelling a predicate callback should settle the task");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true), Value::int(1)])
    );
}
