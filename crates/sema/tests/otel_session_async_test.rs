//! Cooperative runtime coverage for `otel/with-session`.

#![cfg(not(target_arch = "wasm32"))]

use sema_core::Value;
use sema_eval::Interpreter;
use sema_vm::runtime::RootOptions;

#[test]
fn with_session_is_cooperative_and_restores_task_local_scope() {
    sema_otel::testing::set_compat("all");
    let cap = sema_otel::testing::install();
    let interp = Interpreter::new();

    let result = interp
        .eval_str_compiled(
            r#"
            (define out (channel/new 4))
            (define scoped
              (async/spawn
                (fn ()
                  (otel/with-session
                    "slow-session"
                    {:user "alice"}
                    (fn ()
                      (channel/send out :started)
                      (async/sleep 100)
                      (otel/span "session-inner" (fn () nil))
                      :done)))))
            (define sibling
              (async/spawn
                (fn ()
                  (async/sleep 10)
                  (otel/span "sibling" (fn () nil))
                  (channel/send out :sibling))))
            (define started (channel/recv out))
            (define first-after-start (channel/recv out))
            (define scoped-result (async/await scoped))
            (async/await sibling)

            (otel/with-session
              "outer-session"
              {:user "outer-user"}
              (fn ()
                (otel/span "outer-before" (fn () nil))
                (otel/with-session
                  "inner-session"
                  42
                  (fn ()
                    (async/sleep 1)
                    (otel/span "nested-inner" (fn () nil))))
                (otel/span "outer-after" (fn () nil))))
            (otel/span "post-session" (fn () nil))

            (try
              (otel/with-session
                "failed-session"
                {:user "failed-user"}
                (fn ()
                  (async/sleep 1)
                  (error "session stopped")))
              (catch error nil))
            (otel/span "after-failure" (fn () nil))

            (define ready (channel/new 1))
            (define cancelled
              (async/spawn
                (fn ()
                  (otel/with-session
                    "cancelled-session"
                    {:user "cancelled-user"}
                    (fn ()
                      (channel/send ready true)
                      (async/sleep 100)
                      (otel/span "must-not-run" (fn () nil)))))))
            (channel/recv ready)
            (async/cancel cancelled)
            (try (async/await cancelled) (catch error nil))
            (otel/span "after-cancel" (fn () nil))

            (define direct-native
              (channel? (otel/with-session "direct-session" channel/new)))
            (list started first-after-start scoped-result direct-native)
            "#,
        )
        .expect("session wrapper should run through the cooperative runtime ABI");

    assert_eq!(
        result,
        Value::list(vec![
            Value::keyword("started"),
            Value::keyword("sibling"),
            Value::keyword("done"),
            Value::bool(true),
        ])
    );

    let root_a = interp
        .submit_str(
            r#"
            (otel/with-session
              "root-a"
              {:user "root-user"}
              (fn ()
                (async/sleep 50)
                (otel/span "root-a-inner" (fn () nil))))
            "#,
            RootOptions::default(),
        )
        .expect("root A submits");
    let root_b = interp
        .submit_str(
            r#"(otel/span "independent-root" (fn () nil))"#,
            RootOptions::default(),
        )
        .expect("root B submits");
    interp.drive_until_settled(&root_a).expect("root A settles");
    interp.drive_until_settled(&root_b).expect("root B settles");

    let spans = cap.spans_json();
    let span = |name: &str| {
        spans
            .iter()
            .find(|candidate| candidate["name"] == name)
            .unwrap_or_else(|| panic!("missing span {name}"))
    };
    let assert_scope = |name: &str, session: &str, user: &str| {
        let attributes = &span(name)["attributes"];
        assert_eq!(attributes["session.id"], session, "{name} session");
        assert_eq!(attributes["user.id"], user, "{name} user");
    };
    let assert_unscoped = |name: &str| {
        let attributes = &span(name)["attributes"];
        assert!(attributes["session.id"].is_null(), "{name} session leaked");
        assert!(attributes["user.id"].is_null(), "{name} user leaked");
    };

    assert_scope("session-inner", "slow-session", "alice");
    assert_scope("outer-before", "outer-session", "outer-user");
    assert_scope("nested-inner", "inner-session", "outer-user");
    assert_scope("outer-after", "outer-session", "outer-user");
    for name in [
        "sibling",
        "post-session",
        "after-failure",
        "after-cancel",
        "independent-root",
    ] {
        assert_unscoped(name);
    }
    assert_scope("root-a-inner", "root-a", "root-user");
    assert!(
        spans
            .iter()
            .all(|candidate| candidate["name"] != "must-not-run"),
        "cancellation must prevent the remainder of the session body"
    );
    assert!(
        sema_otel::current_conversation_id().is_none(),
        "the runtime thread must be unscoped after settlement"
    );
}
