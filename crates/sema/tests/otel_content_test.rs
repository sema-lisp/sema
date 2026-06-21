//! M3 privacy: with the standard content-capture flag set, the LLM span carries
//! gen_ai.input.messages / gen_ai.output.messages. Own process so the env flag and
//! global provider are isolated from the privacy-default test.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;

#[test]
fn content_capture_when_opted_in() {
    // SAFETY: single-threaded test setup before any LLM call.
    unsafe {
        std::env::set_var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "true");
    }
    let cap = sema_otel::testing::install();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply_with_usage("the answer is 42", 12, 5)
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    interp
        .eval_str_compiled(r#"(llm/complete "what is the answer?" {:max-tokens 10})"#)
        .expect("completion should run");

    let span = cap
        .span_named("chat fake-model")
        .expect("a chat span should be emitted");
    let input = span["attributes"]["gen_ai.input.messages"]
        .as_str()
        .expect("input messages captured");
    let output = span["attributes"]["gen_ai.output.messages"]
        .as_str()
        .expect("output messages captured");
    assert!(input.contains("what is the answer?"), "input: {input}");
    assert!(output.contains("the answer is 42"), "output: {output}");
}
