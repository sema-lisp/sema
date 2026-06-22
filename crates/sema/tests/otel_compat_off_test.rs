//! CRITICAL: with SEMA_OTEL_COMPAT unset, NONE of the backend alias keys are emitted —
//! spans carry only the canonical gen_ai.* / sema.* / session.id / user.id /
//! langfuse.observation.* set. Own binary (process-global state).

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;

#[test]
fn compat_off_emits_no_aliases() {
    unsafe {
        std::env::set_var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "true");
    }
    // Default: compat off (no set_compat call).
    let cap = sema_otel::testing::install();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply_with_usage("hi", 10, 5)
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    interp
        .eval_str_compiled(r#"(llm/complete "q" {:max-tokens 8})"#)
        .expect("completion");

    let chat = cap.span_named("chat fake-model").expect("a chat span");
    let attrs = &chat["attributes"];

    // None of the compat alias keys may appear.
    for forbidden in [
        "openinference.span.kind",
        "llm.model_name",
        "llm.provider",
        "llm.system",
        "llm.token_count.prompt",
        "llm.cost.total",
        "llm.invocation_parameters",
        "input.value",
        "output.value",
        "input.mime_type",
        "traceloop.span.kind",
        "traceloop.entity.input",
        "llm.usage.total_tokens",
        "gen_ai.usage.prompt_tokens",
        "langsmith.span.kind",
        "gen_ai.system",
        "langfuse.observation.type",
        "langfuse.observation.model.name",
        "langfuse.observation.usage_details",
        "braintrust.metrics",
    ] {
        assert!(
            attrs.get(forbidden).is_none(),
            "compat OFF must not emit alias key `{forbidden}`"
        );
    }

    // Canonical attrs ARE still present.
    assert_eq!(attrs["gen_ai.usage.input_tokens"], 10);
    assert_eq!(attrs["gen_ai.provider.name"], "fake");
}
