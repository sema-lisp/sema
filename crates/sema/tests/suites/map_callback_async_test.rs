//! Cooperative-callback coverage for map traversal higher-order functions.

use sema_core::Value;
use sema_eval::Interpreter;

fn eval(source: &str) -> Result<Value, sema_core::SemaError> {
    Interpreter::new().eval_str_compiled(source)
}

#[test]
fn map_values_and_filter_callbacks_suspend_in_entry_order() {
    let result = eval(
        r#"
        (let ((out (channel/new 8)))
          (let ((mapped
                  (map/map-vals
                    (fn (value)
                      (channel/send out value)
                      (async/sleep 1)
                      (+ value 10))
                    {:a 1 :b 2}))
                (filtered
                  (map/filter
                    (fn (key value)
                      (channel/send out key)
                      (async/sleep 1)
                      (> value 1))
                    {:a 1 :b 2})))
            (list
              (:a mapped)
              (:b mapped)
              (count filtered)
              (:b filtered)
              (list
                (channel/recv out) (channel/recv out)
                (channel/recv out) (channel/recv out)))))
        "#,
    )
    .expect("map traversal callbacks should run through the runtime ABI");

    assert_eq!(
        result,
        Value::list(vec![
            Value::int(11),
            Value::int(12),
            Value::int(1),
            Value::int(2),
            Value::list(vec![
                Value::int(1),
                Value::int(2),
                Value::keyword("a"),
                Value::keyword("b"),
            ]),
        ])
    );
}

#[test]
fn map_keys_suspends_and_preserves_collision_semantics() {
    let result = eval(
        r#"
        (let ((seen 0))
          (let ((mapped
                  (map/map-keys
                    (fn (key)
                      (set! seen (+ seen 1))
                      (async/sleep 1)
                      :same)
                    {:a 1 :b 2})))
            (list seen (count mapped) (:same mapped))))
        "#,
    )
    .expect("map/map-keys callback should suspend cooperatively");

    assert_eq!(
        result,
        Value::list(vec![Value::int(2), Value::int(1), Value::int(2)])
    );
}

#[test]
fn map_traversals_preserve_map_kind_and_support_a_runtime_native_callback() {
    let result = eval(
        r#"
        (let ((mapped (map/map-vals async/resolved (hashmap/new :x 7))))
          (list
            (type (map/map-vals (fn (value) value) {}))
            (type (map/filter (fn (key value) #t) {}))
            (type (map/map-keys (fn (key) key) {}))
            (type mapped)
            (type (map/filter (fn (key value) #t) (hashmap/new)))
            (type (map/map-keys (fn (key) key) (hashmap/new)))
            (async/resolved? (:x mapped))))
        "#,
    )
    .expect("map traversals should preserve sorted and hashed map kinds");

    assert_eq!(
        result,
        Value::list(vec![
            Value::keyword("map"),
            Value::keyword("map"),
            Value::keyword("map"),
            Value::keyword("hashmap"),
            Value::keyword("hashmap"),
            Value::keyword("hashmap"),
            Value::bool(true),
        ])
    );
}

#[test]
fn hashmap_nan_entries_survive_every_async_map_traversal() {
    let result = eval(
        r#"
        (let ((out (channel/new 8)))
          (let ((mapped
                  (map/map-vals
                    (fn (value)
                      (channel/send out value)
                      (async/sleep 1)
                      (+ value 1))
                    (hashmap/new math/nan 7)))
                (filtered
                  (map/filter
                    (fn (key value)
                      (channel/send out value)
                      (async/sleep 1)
                      #t)
                    (hashmap/new math/nan 7)))
                (rekeyed
                  (map/map-keys
                    (fn (key)
                      (channel/send out "key")
                      (async/sleep 1)
                      :safe)
                    (hashmap/new math/nan 7))))
            (list
              (first (vals mapped))
              (first (vals filtered))
              (:safe rekeyed)
              (count mapped)
              (count filtered)
              (count rekeyed)
              (list
                (channel/recv out)
                (channel/recv out)
                (channel/recv out)))))
        "#,
    )
    .expect("hashmap traversal should not re-lookup non-reflexive keys");

    assert_eq!(
        result,
        Value::list(vec![
            Value::int(8),
            Value::int(7),
            Value::int(7),
            Value::int(1),
            Value::int(1),
            Value::int(1),
            Value::list(vec![Value::int(7), Value::int(7), Value::string("key"),]),
        ])
    );
}

#[test]
fn map_traversal_callback_observes_the_callers_task_context() {
    let result = eval(
        r#"
        (context/with
          {:minimum 2}
          (fn ()
            (map/filter
              (fn (key value)
                (async/sleep 1)
                (>= value (context/get :minimum)))
              {:a 1 :b 2 :c 3})))
        "#,
    )
    .expect("map callback should inherit task context");

    assert_eq!(
        result,
        eval("{:b 2 :c 3}").expect("expected map should evaluate")
    );
}

#[test]
fn map_traversal_callback_parks_while_a_sibling_runs() {
    let result = eval(
        r#"
        (let ((out (channel/new 4)))
          (let ((mapping
                  (async/spawn
                    (fn ()
                      (map/map-vals
                        (fn (value)
                          (async/sleep 100)
                          (channel/send out "mapping")
                          value)
                        {:a 1}))))
                (sibling
                  (async/spawn
                    (fn ()
                      (async/sleep 10)
                      (channel/send out "sibling")))))
            (let ((first (channel/recv out)))
              (async/await mapping)
              (async/await sibling)
              first)))
        "#,
    )
    .expect("a parked map callback should yield to sibling tasks");

    assert_eq!(result.as_str(), Some("sibling"));
}

#[test]
fn map_traversal_failure_stops_before_later_entries() {
    let result = eval(
        r#"
        (let ((seen 0))
          (try
            (map/filter
              (fn (key value)
                (set! seen (+ seen 1))
                (async/sleep 1)
                (if (= value 2) (error "stop") #t))
              {:a 1 :b 2 :c 3})
            (catch error nil))
          seen)
        "#,
    )
    .expect("map callback failure should be catchable");

    assert_eq!(result.as_int(), Some(2));

    let invalid_key = eval(
        r#"
        (let ((seen 0))
          (try
            (map/map-keys
              (fn (key)
                (set! seen (+ seen 1))
                (async/sleep 1)
                (mutable-cell/new key))
              {:a 1 :b 2})
            (catch error nil))
          seen)
        "#,
    )
    .expect("invalid mapped key should be catchable");

    assert_eq!(invalid_key.as_int(), Some(1));
}

#[test]
fn cancelling_a_map_traversal_callback_stops_before_later_entries() {
    let result = eval(
        r#"
        (let ((seen 0))
          (let ((pending
                  (async/spawn
                    (fn ()
                      (map/map-vals
                        (fn (value)
                          (set! seen (+ seen 1))
                          (async/sleep 100)
                          value)
                        {:a 1 :b 2})))))
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
    .expect("cancelling a map callback should settle the task");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true), Value::int(1)])
    );
}
