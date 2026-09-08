//! Cooperative-callback coverage for map update operations.

use sema_core::Value;
use sema_eval::Interpreter;

fn eval(source: &str) -> Result<Value, sema_core::SemaError> {
    Interpreter::new().eval_str_compiled(source)
}

#[test]
fn map_update_callback_suspends_for_sorted_hash_and_missing_values() {
    let result = eval(
        r#"
        (let ((out (channel/new 8)))
          (let ((sorted
                  (map/update
                    {:x 1}
                    :x
                    (fn (current)
                      (channel/send out current)
                      (async/sleep 1)
                      (+ current 10))))
                (hashed
                  (map/update
                    (hashmap/new :x 2)
                    :x
                    (fn (current)
                      (channel/send out current)
                      (async/sleep 1)
                      (+ current 10))))
                (missing
                  (map/update
                    {}
                    :x
                    (fn (current)
                      (channel/send out current)
                      (async/sleep 1)
                      99))))
            (list
              (type sorted) (:x sorted)
              (type hashed) (:x hashed)
              (:x missing)
              (list
                (channel/recv out)
                (channel/recv out)
                (channel/recv out)))))
        "#,
    )
    .expect("map/update callback should run through the runtime ABI");

    assert_eq!(
        result,
        Value::list(vec![
            Value::keyword("map"),
            Value::int(11),
            Value::keyword("hashmap"),
            Value::int(12),
            Value::int(99),
            Value::list(vec![Value::int(1), Value::int(2), Value::nil()]),
        ])
    );
}

#[test]
fn map_updates_support_a_direct_runtime_native_callback() {
    let result = eval(
        r#"
        (let ((updated (map/update {} :value async/resolved))
              (nested (map/update-in {} (list :outer :value) async/resolved)))
          (list
            (async/resolved? (:value updated))
            (async/resolved? (:value (:outer nested)))))
        "#,
    )
    .expect("map updates should structurally invoke runtime natives");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true)])
    );
}

#[test]
fn update_in_preserves_nested_map_kinds_and_builds_missing_maps() {
    let result = eval(
        r#"
        (let ((updated
                (update-in
                  {:outer (hashmap/new :value 1)}
                  (list :outer :value)
                  (fn (current)
                    (async/sleep 1)
                    (+ current 1))))
              (created
                (map/update-in
                  {}
                  (list :outer :value)
                  (fn (current)
                    (async/sleep 1)
                    9))))
          (list
            (type updated)
            (type (:outer updated))
            (:value (:outer updated))
            (type (:outer created))
            (:value (:outer created))))
        "#,
    )
    .expect("update-in should rebuild each nested map after suspension");

    assert_eq!(
        result,
        Value::list(vec![
            Value::keyword("map"),
            Value::keyword("hashmap"),
            Value::int(2),
            Value::keyword("map"),
            Value::int(9),
        ])
    );
}

#[test]
fn map_update_preserves_value_identity_and_update_in_empty_path_context() {
    let result = eval(
        r#"
        (context/with
          {:increment 10}
          (fn ()
            (let ((cell (mutable-cell/new 1))
                  (seen (mutable-cell/new nil)))
              (let ((updated
                      (map/update
                        {:value cell}
                        :value
                        (fn (current)
                          (mutable-cell/set! seen current)
                          (async/sleep 1)
                          current)))
                    (whole
                      (update-in
                        5
                        (list)
                        (fn (current)
                          (async/sleep 1)
                          (+ current (context/get :increment))))))
                (list
                  (eq? (mutable-cell/get seen) cell)
                  (eq? (:value updated) cell)
                  whole)))))
        "#,
    )
    .expect("map updates should preserve identity and caller context");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true), Value::int(15)])
    );
}

#[test]
fn map_update_callback_parks_while_a_sibling_runs() {
    let result = eval(
        r#"
        (let ((out (channel/new 4)))
          (let ((updating
                  (async/spawn
                    (fn ()
                      (update-in
                        {:value 1}
                        (list :value)
                        (fn (current)
                          (async/sleep 100)
                          (channel/send out "updating")
                          current)))))
                (sibling
                  (async/spawn
                    (fn ()
                      (async/sleep 10)
                      (channel/send out "sibling")))))
            (let ((first (channel/recv out)))
              (async/await updating)
              (async/await sibling)
              first)))
        "#,
    )
    .expect("a parked map update should yield to sibling tasks");

    assert_eq!(result.as_str(), Some("sibling"));
}

#[test]
fn map_update_failure_is_catchable_and_invalid_paths_do_not_call_back() {
    let result = eval(
        r#"
        (let ((seen 0))
          (try
            (update-in
              {:value 1}
              (list :value)
              (fn (current)
                (set! seen (+ seen 1))
                (async/sleep 1)
                (error "stop")))
            (catch error nil))
          (try
            (update-in
              {}
              (list (mutable-cell/new :invalid))
              (fn (current) (set! seen (+ seen 100)) current))
            (catch error nil))
          seen)
        "#,
    )
    .expect("update failure and path validation should be catchable");

    assert_eq!(result.as_int(), Some(1));
}

#[test]
fn cancelling_a_map_update_settles_the_task() {
    let result = eval(
        r#"
        (let ((seen 0))
          (let ((pending
                  (async/spawn
                    (fn ()
                      (map/update
                        {:value 1}
                        :value
                        (fn (current)
                          (set! seen (+ seen 1))
                          (async/sleep 100)
                          current))))))
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
    .expect("cancelling a map update should settle the task");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true), Value::int(1)])
    );
}
