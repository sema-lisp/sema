//! Gap-2: a failed completion records error.type + Error status on the chat span.
//! Own binary (global provider is process-global).

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;
use sema_llm::types::LlmError;

#[test]
fn provider_error_tags_span_error() {
    let cap = sema_otel::testing::install();

    // Non-retryable 400s fail fast (no retries). Cover both the default-provider
    // path and explicit fallback-chain exhaustion: both are provider-dispatch
    // failures at the public chat-span boundary.
    let p1 = FakeProvider::builder("p1")
        .model("fake-model")
        .error(LlmError::Api {
            status: 400,
            message: "bad request".into(),
        })
        .build();
    let p2 = FakeProvider::builder("p2")
        .model("fake-model")
        .error(LlmError::Api {
            status: 400,
            message: "bad request".into(),
        })
        .error(LlmError::Api {
            status: 400,
            message: "bad request".into(),
        })
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(p1));
    register_test_provider(Box::new(p2));

    let direct = interp.eval_str_compiled(r#"(llm/complete "direct")"#);
    assert!(
        direct.is_err(),
        "the default-provider completion should fail"
    );
    let fallback = interp
        .eval_str_compiled(r#"(llm/with-fallback [:p1 :p2] (fn () (llm/complete "fallback")))"#);
    assert!(fallback.is_err(), "the fallback chain should be exhausted");

    let chats: Vec<_> = cap
        .spans_json()
        .into_iter()
        .filter(|s| s["attributes"]["gen_ai.operation.name"] == "chat")
        .collect();
    assert_eq!(chats.len(), 2, "both failed calls should emit a chat span");
    for chat in chats {
        assert_eq!(chat["attributes"]["error.type"], "provider_error");
        assert!(
            chat["status"]
                .as_str()
                .is_some_and(|s| s.starts_with("error")),
            "span status should be Error, got {:?}",
            chat["status"]
        );
    }
}
