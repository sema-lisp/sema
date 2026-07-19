//! Cooperative-callback coverage for collecting higher-order functions.

use sema_core::Value;
use sema_eval::Interpreter;

fn eval(source: &str) -> Result<Value, sema_core::SemaError> {
    Interpreter::new().eval_str_compiled(source)
}

#[test]
fn map_indexed_callback_can_use_runtime_only_operations() {
    let result = eval(
        r#"
        (let ((out (channel/new 4)))
          (let ((mapped
                  (map-indexed
                    (fn (index item)
                      (async/sleep 1)
                      (channel/send out item)
                      (+ index item))
                    (list 10 20))))
            (list mapped (channel/recv out) (channel/recv out))))
        "#,
    )
    .expect("map-indexed callback should run through the runtime ABI");

    assert_eq!(
        result,
        Value::list(vec![
            Value::list(vec![Value::int(10), Value::int(21)]),
            Value::int(10),
            Value::int(20),
        ])
    );
}

#[test]
fn collecting_callback_parks_while_a_sibling_runs() {
    let result = eval(
        r#"
        (let ((out (channel/new 4)))
          (let ((mapped
                  (async/spawn
                    (fn ()
                      (map-indexed
                        (fn (index item)
                          (async/sleep 100)
                          (channel/send out "mapped")
                          (+ index item))
                        (list 10)))))
                (sibling
                  (async/spawn
                    (fn ()
                      (async/sleep 10)
                      (channel/send out "sibling")))))
            (let ((first (channel/recv out)))
              (async/await mapped)
              (async/await sibling)
              first)))
        "#,
    )
    .expect("collecting callback should park cooperatively");

    assert_eq!(result.as_str(), Some("sibling"));
}

#[test]
fn collecting_callback_observes_the_callers_task_context() {
    let result = eval(
        r#"
        (context/with
          {:tag "task"}
          (fn ()
            (map-indexed
              (fn (index item) (context/get :tag))
              (list 1))))
        "#,
    )
    .expect("collecting callback should inherit task context");

    assert_eq!(result, Value::list(vec![Value::string("task")]));
}

#[test]
fn list_collectors_preserve_their_distinct_output_semantics() {
    let result = eval(
        r#"
        (let ((out (channel/new 8)))
          (list
            (flat-map
              (fn (item)
                (channel/send out item)
                (async/sleep 1)
                (list item (* item 10)))
              (list 1 2))
            (list/times
              3
              (fn (index)
                (channel/send out index)
                (async/sleep 1)
                (* index index)))
            (list
              (channel/recv out)
              (channel/recv out)
              (channel/recv out)
              (channel/recv out)
              (channel/recv out))))
        "#,
    )
    .expect("flat-map and list/times callbacks should suspend cooperatively");

    assert_eq!(
        result,
        Value::list(vec![
            Value::list(vec![
                Value::int(1),
                Value::int(10),
                Value::int(2),
                Value::int(20),
            ]),
            Value::list(vec![Value::int(0), Value::int(1), Value::int(4)]),
            Value::list(vec![
                Value::int(1),
                Value::int(2),
                Value::int(0),
                Value::int(1),
                Value::int(2),
            ]),
        ])
    );
}

#[test]
fn string_and_typed_array_maps_suspend_and_validate_results() {
    let result = eval(
        r#"
        (let ((out (channel/new 8)))
          (list
            (string/map
              (fn (ch)
                (channel/send out ch)
                (async/sleep 1)
                (char-upcase ch))
              "ab")
            (f64-array/sum
              (f64-array/map
                (fn (number)
                  (channel/send out number)
                  (async/sleep 1)
                  (* number 2.0))
                (f64-array/from-list (list 1.5 2.5))))
            (i64-array/sum
              (i64-array/map
                (fn (number)
                  (channel/send out number)
                  (async/sleep 1)
                  (* number number))
                (i64-array 2 3)))
            (list
              (channel/recv out)
              (channel/recv out)
              (channel/recv out)
              (channel/recv out)
              (channel/recv out)
              (channel/recv out))))
        "#,
    )
    .expect("string and typed-array callbacks should suspend cooperatively");

    assert_eq!(
        result,
        Value::list(vec![
            Value::string("AB"),
            Value::float(8.0),
            Value::int(13),
            Value::list(vec![
                Value::char('a'),
                Value::char('b'),
                Value::float(1.5),
                Value::float(2.5),
                Value::int(2),
                Value::int(3),
            ]),
        ])
    );

    let invalid = eval(
        r#"
        (let ((seen 0))
          (try
            (i64-array/map
              (fn (number)
                (set! seen (+ seen 1))
                (async/sleep 1)
                "not-an-integer")
              (i64-array 1 2))
            (catch error nil))
          seen)
        "#,
    )
    .expect("invalid callback result should be catchable");
    assert_eq!(invalid.as_int(), Some(1));
}

#[test]
fn collector_output_modes_preserve_legacy_coercions_and_empty_values() {
    let result = eval(
        r#"
        (list
          (flat-map
            (fn (item)
              (async/sleep 1)
              (if (= item 1) (vector 1 10) 2))
            (list 1 2))
          (string/map
            (fn (ch)
              (async/sleep 1)
              (str "<" (char->string ch) ">"))
            "ab")
          (f64-array/sum
            (f64-array/map
              (fn (number)
                (async/sleep 1)
                2)
              (f64-array/from-list (list 1.0 2.0))))
          (list
            (flat-map (fn (item) item) (list))
            (string/map (fn (ch) ch) "")
            (f64-array/length (f64-array/map (fn (number) number) (f64-array)))
            (i64-array/length (i64-array/map (fn (number) number) (i64-array)))))
        "#,
    )
    .expect("collector output modes should preserve legacy semantics");

    assert_eq!(
        result,
        Value::list(vec![
            Value::list(vec![Value::int(1), Value::int(10), Value::int(2)]),
            Value::string("<a><b>"),
            Value::float(4.0),
            Value::list(vec![
                Value::list(Vec::new()),
                Value::string(""),
                Value::int(0),
                Value::int(0),
            ]),
        ])
    );
}

#[test]
fn cancelling_a_collecting_callback_stops_before_later_items() {
    let result = eval(
        r#"
        (let ((seen 0))
          (let ((pending
                  (async/spawn
                    (fn ()
                      (list/times
                        2
                        (fn (index)
                          (set! seen (+ seen 1))
                          (async/sleep 100)
                          index))))))
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
    .expect("cancelling a collecting callback should settle the task");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true), Value::int(1)])
    );
}
