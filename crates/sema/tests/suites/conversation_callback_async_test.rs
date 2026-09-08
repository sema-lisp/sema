//! Cooperative-callback coverage for conversation higher-order helpers.
//!
//! These callbacks run on the active runtime task so sleep/channel waits park
//! that task, preserve input order, and remain cancellable between messages.

use sema_core::Value;
use sema_eval::Interpreter;

fn eval(source: &str) -> Result<Value, sema_core::SemaError> {
    Interpreter::new().eval_str_compiled(source)
}

#[test]
fn conversation_map_callback_parks_while_a_sibling_runs() {
    let result = eval(
        r#"
        (let ((out (channel/new 4))
              (conv (-> (conversation/new)
                        (conversation/add-message :user "one")
                        (conversation/add-message :assistant "two"))))
          (let ((mapped
                  (async/spawn
                    (fn ()
                      (conversation/map
                        conv
                        (fn (msg)
                          (async/sleep 100)
                          (channel/send out (message/content msg))
                          (message/content msg))))))
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
    .expect("conversation/map callback should suspend cooperatively");

    assert_eq!(result.as_str(), Some("sibling"));
}

#[test]
fn conversation_callbacks_resume_in_order_for_every_helper() {
    let result = eval(
        r#"
        (let ((conv (-> (conversation/new)
                        (conversation/add-message :user "u1")
                        (conversation/add-message :assistant "a1")
                        (conversation/add-message :user "u2"))))
          (list
            (conversation/map
              conv
              (fn (msg)
                (async/sleep 1)
                (message/content msg)))
            (conversation/map
              (conversation/filter
                conv
                (fn (msg)
                  (async/sleep 1)
                  (= (message/role msg) :user)))
              message/content)
            (conversation/map
              (conversation/map-role
                conv
                :user
                (fn (msg)
                  (async/sleep 1)
                  (message :user (string/upper (message/content msg)))))
              message/content)
            (message/content
              (conversation/find
                conv
                (fn (msg)
                  (async/sleep 1)
                  (= (message/role msg) :assistant))))))
        "#,
    )
    .expect("all conversation callbacks should resume cooperatively");

    assert_eq!(
        result,
        Value::list(vec![
            Value::list(vec![
                Value::string("u1"),
                Value::string("a1"),
                Value::string("u2"),
            ]),
            Value::list(vec![Value::string("u1"), Value::string("u2")]),
            Value::list(vec![
                Value::string("U1"),
                Value::string("a1"),
                Value::string("U2"),
            ]),
            Value::string("a1"),
        ])
    );
}

#[test]
fn conversation_callback_observes_the_callers_task_context() {
    let result = eval(
        r#"
        (context/with
          {:request-id "req-42"}
          (fn ()
            (conversation/map
              (conversation/add-message (conversation/new) :user "hello")
              (fn (msg)
                (async/sleep 1)
                (context/get :request-id)))))
        "#,
    )
    .expect("conversation callback should inherit the caller's task context");

    assert_eq!(result, Value::list(vec![Value::string("req-42")]));
}

#[test]
fn conversation_find_returns_the_exact_message_seen_by_the_predicate() {
    let result = eval(
        r#"
        (let ((seen (mutable-cell/new nil))
              (conv (conversation/add-message (conversation/new) :user "hello")))
          (let ((found
                  (conversation/find
                    conv
                    (fn (msg)
                      (mutable-cell/set! seen msg)
                      (async/sleep 1)
                      #t))))
            (eq? found (mutable-cell/get seen))))
        "#,
    )
    .expect("conversation/find identity program should evaluate");

    assert_eq!(result, Value::bool(true));
}

#[test]
fn conversation_callback_failure_stops_before_later_messages() {
    let result = eval(
        r#"
        (let ((seen 0)
              (conv (-> (conversation/new)
                        (conversation/add-message :user "one")
                        (conversation/add-message :user "two")
                        (conversation/add-message :user "three"))))
          (try
            (conversation/map
              conv
              (fn (msg)
                (set! seen (+ seen 1))
                (async/sleep 1)
                (if (= (message/content msg) "two")
                  (error "stop")
                  (message/content msg))))
            (catch error nil))
          seen)
        "#,
    )
    .expect("callback failure should be catchable by the caller");

    assert_eq!(result.as_int(), Some(2));
}

#[test]
fn cancelling_conversation_callback_stops_the_sequence() {
    let result = eval(
        r#"
        (let ((seen 0)
              (conv (-> (conversation/new)
                        (conversation/add-message :user "one")
                        (conversation/add-message :user "two"))))
          (let ((pending
                  (async/spawn
                    (fn ()
                      (conversation/filter
                        conv
                        (fn (msg)
                          (set! seen (+ seen 1))
                          (async/sleep 100)
                          #t))))))
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
    .expect("cancelling a parked callback should settle the task");

    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(true), Value::int(1)])
    );
}
