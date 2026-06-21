//! M4 acceptance: `llm/embed` emits an `embeddings` CLIENT span with input tokens
//! only. Deterministic (FakeProvider + in-memory exporter). Own binary.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;

#[test]
fn embed_emits_embeddings_span() {
    let cap = sema_otel::testing::install();

    let fake = FakeProvider::builder("fake")
        .model("fake-embed")
        .embed(vec![vec![0.1, 0.2, 0.3]])
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    interp
        .eval_str_compiled(r#"(llm/embed "hello world")"#)
        .expect("embed should run against the fake");

    let spans = cap.spans_json();
    let embed = spans
        .iter()
        .find(|s| s["attributes"]["gen_ai.operation.name"] == "embeddings")
        .expect("an embeddings span");
    assert_eq!(embed["kind"], "client");
    assert_eq!(embed["attributes"]["gen_ai.provider.name"], "fake");
    assert_eq!(embed["attributes"]["gen_ai.response.model"], "fake-embed");
    // Embeddings report input tokens only.
    assert_eq!(embed["attributes"]["gen_ai.usage.input_tokens"], 1);
    assert_eq!(embed["attributes"]["gen_ai.usage.output_tokens"], 0);
}
