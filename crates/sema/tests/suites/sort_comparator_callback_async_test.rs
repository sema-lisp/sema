//! Cooperative-callback coverage for the two-argument `sort` comparator.

use sema_core::Value;
use sema_eval::Interpreter;

fn eval(source: &str) -> Result<Value, sema_core::SemaError> {
    Interpreter::new().eval_str_compiled(source)
}

#[test]
fn sort_comparator_suspends_and_preserves_stable_result_semantics() {
    let result = eval(
        r#"
        (let ((out (channel/new 16))
              (items
                (list
                  {:key 2 :id "first-two"}
                  {:key 1 :id "one"}
                  {:key 2 :id "second-two"})))
          (let ((by-key
                  (sort
                    items
                    (fn (a b)
                      (channel/send out (list (:id a) (:id b)))
                      (async/sleep 1)
                      (- (:key a) (:key b)))))
                (boolean-order
                  (sort
                    (list 3 1 2)
                    (fn (a b)
                      (async/sleep 1)
                      (< a b))))
                (all-equal
                  (sort
                    (list "a" "b" "c")
                    (fn (a b)
                      (async/sleep 1)
                      "not-an-order"))))
            (list (map :id by-key) boolean-order all-equal)))
        "#,
    )
    .expect("sort comparators should run through the runtime ABI");

    assert_eq!(
        result,
        Value::list(vec![
            Value::list(vec![
                Value::string("one"),
                Value::string("first-two"),
                Value::string("second-two"),
            ]),
            Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
            Value::list(vec![
                Value::string("a"),
                Value::string("b"),
                Value::string("c"),
            ]),
        ])
    );
}

#[test]
fn sort_comparator_handles_large_unpaired_passes_and_trivial_inputs() {
    let result = eval(
        r#"
        (let ((expected (range 0 257)))
          (list
            (= (sort (reverse expected) -) expected)
            (sort (list) 42)
            (sort (list "only") 42)))
        "#,
    )
    .expect("bottom-up merge passes should handle non-power-of-two input lengths");

    assert_eq!(
        result,
        Value::list(vec![
            Value::bool(true),
            Value::list(Vec::new()),
            Value::list(vec![Value::string("only")]),
        ])
    );
}

#[test]
fn sort_comparator_observes_the_callers_task_context() {
    let result = eval(
        r#"
        (context/with
          {:direction -1}
          (fn ()
            (sort
              (list 1 3 2)
              (fn (a b)
                (async/sleep 1)
                (* (context/get :direction) (- a b))))))
        "#,
    )
    .expect("sort comparator should inherit task context");

    assert_eq!(
        result,
        Value::list(vec![Value::int(3), Value::int(2), Value::int(1)])
    );
}

#[test]
fn a_parked_sort_comparison_yields_to_a_sibling() {
    let result = eval(
        r#"
        (let ((out (channel/new 4)))
          (let ((sorting
                  (async/spawn
                    (fn ()
                      (sort
                        (list 2 1)
                        (fn (a b)
                          (async/sleep 100)
                          (channel/send out "sort")
                          (- a b))))))
                (sibling
                  (async/spawn
                    (fn ()
                      (async/sleep 10)
                      (channel/send out "sibling")))))
            (let ((first (channel/recv out)))
              (async/await sorting)
              (async/await sibling)
              first)))
        "#,
    )
    .expect("a parked sort comparator should yield to sibling tasks");

    assert_eq!(result.as_str(), Some("sibling"));
}

#[test]
fn sort_comparator_failure_is_catchable_and_fail_fast() {
    let result = eval(
        r#"
        (let ((seen 0))
          (let ((caught
                  (try
                    (sort
                      (list 3 2 1)
                      (fn (a b)
                        (set! seen (+ seen 1))
                        (async/sleep 1)
                        (error "stop sorting")))
                    (catch error "caught"))))
            (list caught seen)))
        "#,
    )
    .expect("sort comparator failures should remain catchable");

    assert_eq!(
        result,
        Value::list(vec![Value::string("caught"), Value::int(1)])
    );
}

#[test]
fn cancelling_a_sort_comparison_stops_before_later_comparisons() {
    let result = eval(
        r#"
        (let ((seen 0))
          (let ((pending
                  (async/spawn
                    (fn ()
                      (sort
                        (list 4 3 2 1)
                        (fn (a b)
                          (set! seen (+ seen 1))
                          (async/sleep 100)
                          (- a b)))))))
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
    .expect("cancelling a sort comparator should settle the task");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true), Value::int(1)])
    );
}

#[test]
fn sort_snapshots_a_mutable_array_before_comparator_reentry() {
    let result = eval(
        r#"
        (let ((items (mutable-array/new)))
          (mutable-array/push! items 3)
          (mutable-array/push! items 1)
          (mutable-array/push! items 2)
          (let ((sorted
                  (sort
                    items
                    (fn (a b)
                      (mutable-array/push! items 99)
                      (async/sleep 1)
                      (- a b)))))
            (list sorted (> (mutable-array/length items) 3))))
        "#,
    )
    .expect("sort should retain the legacy mutable-array snapshot semantics");

    assert_eq!(
        result,
        Value::list(vec![
            Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
            Value::bool(true),
        ])
    );
}
