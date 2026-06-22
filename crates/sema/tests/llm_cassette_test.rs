//! LLM cassette (record/replay) tests — keyless, deterministic, offline.
//!
//! These prove the cassette folds correctly into the features it sits beside:
//! - replay returns the recorded content (no provider call),
//! - replay reports the *recorded* usage (cost/budget accounting stays exercised),
//! - replay still emits the OpenTelemetry `chat` span with the recorded tokens,
//! - a `:replay` miss is a hard error,
//! - the tape never stores the prompt text (redaction by construction).

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;

/// Unique tape path per test so parallel tests don't collide.
fn tape_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "sema-cassette-{}-{}.jsonl",
        std::process::id(),
        name
    ))
}

/// Run `src` against a fresh interpreter with `fake` installed as the default provider.
fn run(src: &str, fake: FakeProvider) -> Result<sema_core::Value, sema_core::SemaError> {
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));
    interp.eval_str_compiled(src)
}

#[test]
fn records_then_replays_without_calling_provider() {
    let path = tape_path("roundtrip");
    let _ = std::fs::remove_file(&path);

    // Record: a fake that replies "first answer" once.
    let rec = FakeProvider::builder("fake")
        .model("m")
        .reply("first answer")
        .build();
    let recorded = run(
        &format!(
            r#"(llm/with-cassette "{}" {{:mode :record}}
                 (fn () (llm/complete "the prompt" {{:model "m"}})))"#,
            path.display()
        ),
        rec,
    )
    .expect("record run should succeed");
    assert_eq!(recorded.as_str(), Some("first answer"));

    // Replay: a fake that would ERROR if the provider were actually called. Getting
    // the recorded answer back proves the tape served it, not the provider.
    let replay_fake = FakeProvider::builder("fake")
        .model("m")
        .error(sema_llm::types::LlmError::Api {
            status: 500,
            message: "provider must not be called on replay".into(),
        })
        .build();
    let replayed = run(
        &format!(
            r#"(llm/with-cassette "{}" {{:mode :replay}}
                 (fn () (llm/complete "the prompt" {{:model "m"}})))"#,
            path.display()
        ),
        replay_fake,
    )
    .expect("replay run should succeed without touching the provider");
    assert_eq!(replayed.as_str(), Some("first answer"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn replay_reports_recorded_usage() {
    let path = tape_path("usage");
    let _ = std::fs::remove_file(&path);

    // Record a reply carrying explicit token usage.
    let rec = FakeProvider::builder("fake")
        .model("m")
        .reply_with_usage("answer", 111, 222)
        .build();
    run(
        &format!(
            r#"(llm/with-cassette "{}" {{:mode :record}}
                 (fn () (llm/complete "p" {{:model "m"}})))"#,
            path.display()
        ),
        rec,
    )
    .expect("record");

    // Replay, then read last-usage — it must reflect the RECORDED tokens (a replay is
    // a stand-in for a real call, unlike a cache hit which reports zero usage).
    let replay_fake = FakeProvider::builder("fake")
        .model("m")
        .reply("ignored")
        .build();
    let usage = run(
        &format!(
            r#"(llm/with-cassette "{}" {{:mode :replay}}
                 (fn ()
                   (llm/complete "p" {{:model "m"}})
                   [(:prompt-tokens (llm/last-usage)) (:completion-tokens (llm/last-usage))]))"#,
            path.display()
        ),
        replay_fake,
    )
    .expect("replay");
    let items = usage.as_seq().expect("vector");
    assert_eq!(
        items[0].as_int(),
        Some(111),
        "replay must report recorded prompt tokens"
    );
    assert_eq!(
        items[1].as_int(),
        Some(222),
        "replay must report recorded completion tokens"
    );

    let _ = std::fs::remove_file(&path);
}

// The "replay still emits an OTel chat span with the recorded tokens" proof lives in
// its own binary (tests/otel_cassette_test.rs) because the OTel testing harness
// installs a *process-global* tracer provider — sibling tests emitting chat spans
// here would pollute the capture.

#[test]
fn replay_miss_is_a_hard_error() {
    let path = tape_path("miss");
    let _ = std::fs::remove_file(&path); // ensure an empty/absent tape

    let fake = FakeProvider::builder("fake")
        .model("m")
        .reply("unused")
        .build();
    let result = run(
        &format!(
            r#"(llm/with-cassette "{}" {{:mode :replay}}
                 (fn () (llm/complete "never recorded" {{:model "m"}})))"#,
            path.display()
        ),
        fake,
    );
    assert!(
        result.is_err(),
        "a replay miss must be a hard error, not a silent provider call"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("cassette miss"),
        "error should name the cassette miss: {msg}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn records_then_replays_a_streamed_completion() {
    let path = tape_path("stream");
    let _ = std::fs::remove_file(&path);

    // Record a streamed call, accumulating the chunks the callback receives.
    let rec = FakeProvider::builder("fake")
        .model("m")
        .reply("streamed answer")
        .build();
    let recorded = run(
        &format!(
            r#"(define out "")
               (llm/with-cassette "{}" {{:mode :record}}
                 (lambda ()
                   (llm/stream "p" (lambda (c) (set! out (string-append out c))) {{:model "m"}})))
               out"#,
            path.display()
        ),
        rec,
    )
    .expect("record stream")
    .as_str()
    .map(String::from)
    .expect("string");
    assert_eq!(recorded, "streamed answer");

    // Replay with a fake that errors if called — the recorded chunks must come back.
    let replay_fake = FakeProvider::builder("fake")
        .model("m")
        .error(sema_llm::types::LlmError::Api {
            status: 500,
            message: "must not stream".into(),
        })
        .build();
    let replayed = run(
        &format!(
            r#"(define out "")
               (llm/with-cassette "{}" {{:mode :replay}}
                 (lambda ()
                   (llm/stream "p" (lambda (c) (set! out (string-append out c))) {{:model "m"}})))
               out"#,
            path.display()
        ),
        replay_fake,
    )
    .expect("replay stream without touching the provider")
    .as_str()
    .map(String::from)
    .expect("string");
    assert_eq!(
        replayed, recorded,
        "replayed chunks must match what was recorded"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn records_then_replays_an_embedding() {
    let path = tape_path("embed");
    let _ = std::fs::remove_file(&path);

    // Record an embedding (FakeProvider scripts the vectors).
    let rec = FakeProvider::builder("fake")
        .model("m")
        .embed(vec![vec![0.1, 0.2, 0.3]])
        .build();
    run(
        &format!(
            r#"(llm/with-cassette "{}" {{:mode :record}}
                 (lambda () (llm/embed "some text" {{:model "m"}})))"#,
            path.display()
        ),
        rec,
    )
    .expect("record embed");

    // Replay with a fake that has no embeddings scripted — it would error if called.
    // Getting a bytevector back proves the tape served it.
    let replay_fake = FakeProvider::builder("fake").model("m").build();
    let replayed = run(
        &format!(
            r#"(llm/with-cassette "{}" {{:mode :replay}}
                 (lambda () (llm/embed "some text" {{:model "m"}})))"#,
            path.display()
        ),
        replay_fake,
    )
    .expect("replay embed without touching the provider");
    assert_eq!(
        replayed.type_name(),
        "bytevector",
        "replay returns the recorded embedding"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn tape_never_stores_the_prompt_text() {
    let path = tape_path("redaction");
    let _ = std::fs::remove_file(&path);

    // A distinctive prompt that must NOT appear on disk (stand-in for any secret a
    // prompt might carry). The tape stores only the response keyed by a hash.
    let secret_prompt = "SUPER-SECRET-PROMPT-marker-9182";
    let rec = FakeProvider::builder("fake").model("m").reply("ok").build();
    run(
        &format!(
            r#"(llm/with-cassette "{}" {{:mode :record}}
                 (fn () (llm/complete "{}" {{:model "m"}})))"#,
            path.display(),
            secret_prompt
        ),
        rec,
    )
    .expect("record");

    let on_disk = std::fs::read_to_string(&path).expect("tape file should exist");
    assert!(
        !on_disk.contains(secret_prompt),
        "the prompt text must never be written to the tape (redaction); got:\n{on_disk}"
    );

    let _ = std::fs::remove_file(&path);
}
