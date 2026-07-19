//! Task-local dynamic scopes must surround native calls and continuation teardown,
//! not only VM quanta.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;

fn zero_id(s: &str) -> bool {
    s.chars().all(|c| c == '0')
}

#[test]
fn native_callback_and_suspended_span_teardown_use_the_tasks_scope() {
    let cap = sema_otel::testing::install();
    let interp = Interpreter::new();
    interp.global_env.set(
        sema_core::intern("__test/task-identity"),
        sema_core::Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            "__test/task-identity",
            |_args| Ok(sema_core::Value::keyword("legacy")),
            |_context, args| {
                if !args.is_empty() {
                    return Err(sema_core::SemaError::arity(
                        "__test/task-identity",
                        "0",
                        args.len(),
                    ));
                }
                Ok(sema_core::runtime::NativeOutcome::Return(
                    sema_core::Value::list(vec![
                        sema_core::Value::bool(sema_core::current_task_id().is_some()),
                        sema_core::Value::bool(sema_core::current_root().is_some()),
                    ]),
                ))
            },
        )),
    );

    let identity = interp
        .eval_str_compiled(
            r#"
            (async/await
              (async/spawn
                (fn ()
                  (apply __test/task-identity (list)))))
            "#,
        )
        .expect("spawned direct native callback evaluated");
    assert_eq!(
        identity,
        sema_core::Value::list(vec![
            sema_core::Value::bool(true),
            sema_core::Value::bool(true),
        ]),
        "spawned direct native callbacks must publish task and root identity"
    );

    interp
        .eval_str_compiled(
            r#"
            (async/await
              (async/spawn
                (fn ()
                  (otel/span
                    "outer"
                    (fn ()
                      (async/sleep 1)
                      (apply otel/set-attribute (list :native-callback 7))))
                  (otel/span "after" (fn () nil))
                  (try
                    (otel/span
                      "failed-outer"
                      (fn ()
                        (async/sleep 1)
                        (error "stop")))
                    (catch error nil))
                  (otel/span "after-failure" (fn () nil)))))

            (let ((ready (channel/new 1)))
              (let ((pending
                      (async/spawn
                        (fn ()
                          (otel/span
                            "cancelled-outer"
                            (fn ()
                              (channel/send ready true)
                              (async/sleep 100)))))))
                (channel/recv ready)
                (async/cancel pending)
                (try (async/await pending) (catch error nil))
                (otel/span "after-cancel" (fn () nil))))
            "#,
        )
        .expect("async span program evaluated");

    let spans = cap.spans_json();
    let outer = spans
        .iter()
        .find(|span| span["name"] == "outer")
        .expect("outer span present");
    let after = spans
        .iter()
        .find(|span| span["name"] == "after")
        .expect("after span present");
    let after_failure = spans
        .iter()
        .find(|span| span["name"] == "after-failure")
        .expect("after-failure span present");
    let after_cancel = spans
        .iter()
        .find(|span| span["name"] == "after-cancel")
        .expect("after-cancel span present");

    assert_eq!(
        outer["attributes"]["native-callback"], 7,
        "a direct native callback must mutate the active task-local span"
    );
    assert!(
        zero_id(after["parent_span_id"].as_str().unwrap_or("")),
        "dropping the suspended wrapper must remove outer before the next span; outer={}, after={after}",
        outer["span_id"]
    );
    assert_ne!(
        after["trace_id"], outer["trace_id"],
        "the post-wrapper span must start a fresh trace"
    );
    for span in [after_failure, after_cancel] {
        assert!(
            zero_id(span["parent_span_id"].as_str().unwrap_or("")),
            "failure/cancellation teardown must restore the task scope: {span}"
        );
    }
}
