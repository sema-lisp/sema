//! Cassette × OpenTelemetry fold: a *replayed* completion must still emit the
//! `chat` span, populated from the recorded model/usage. Isolated in its own test
//! binary because `sema_otel::testing::install()` sets a process-global tracer
//! provider (sibling tests emitting spans would pollute the capture).

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;

fn run(src: &str, fake: FakeProvider) -> Result<sema_core::Value, sema_core::SemaError> {
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));
    interp.eval_str_compiled(src)
}

#[test]
fn replay_emits_chat_span_with_recorded_tokens() {
    let path =
        std::env::temp_dir().join(format!("sema-cassette-otel-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);

    // Record a reply with explicit usage (no capture installed yet).
    let rec = FakeProvider::builder("fake")
        .model("otel-model")
        .reply_with_usage("answer", 7, 9)
        .build();
    run(
        &format!(
            r#"(llm/with-cassette "{}" {{:mode :record}}
                 (fn () (llm/complete "p" {{:model "otel-model"}})))"#,
            path.display()
        ),
        rec,
    )
    .expect("record");

    // Replay with the in-memory exporter installed. A replay makes no provider call,
    // yet the call still flows through `do_complete`, so the span is emitted.
    let cap = sema_otel::testing::install();
    let replay_fake = FakeProvider::builder("fake")
        .model("otel-model")
        .reply("ignored")
        .build();
    run(
        &format!(
            r#"(llm/with-cassette "{}" {{:mode :replay}}
                 (fn () (llm/complete "p" {{:model "otel-model"}})))"#,
            path.display()
        ),
        replay_fake,
    )
    .expect("replay");

    let spans = cap.spans_json();
    let chat = spans
        .iter()
        .find(|s| s["attributes"]["gen_ai.operation.name"] == "chat")
        .expect("a chat span must be emitted on replay");
    assert_eq!(
        chat["attributes"]["gen_ai.usage.input_tokens"], 7,
        "replay span carries the recorded input tokens"
    );
    assert_eq!(
        chat["attributes"]["gen_ai.usage.output_tokens"], 9,
        "replay span carries the recorded output tokens"
    );
    assert_eq!(
        chat["attributes"]["gen_ai.response.model"], "otel-model",
        "replay span carries the recorded model"
    );

    let _ = std::fs::remove_file(&path);
}
