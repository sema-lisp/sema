//! Root-main LLM calls must park structurally just like spawned tasks. These
//! tests use channel order, not wall-clock thresholds, to prove a sibling ran
//! while the root was waiting on provider or pacing work.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{
    register_test_provider, reset_runtime_state, set_network_max_retries, set_retry_base_ms,
};
use sema_llm::fake::FakeProvider;
use sema_llm::types::LlmError;
use serial_test::serial;
use std::time::{Duration, Instant};

fn strings(value: &sema_core::Value) -> Vec<String> {
    value
        .as_list()
        .expect("list result")
        .iter()
        .map(|value| value.as_str().expect("string result").to_string())
        .collect()
}

#[test]
#[serial]
fn root_completion_parks_while_a_sibling_runs() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(100)
        .reply("provider")
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (let ((out (channel/new 2)))
              (async/spawn (fn () (sleep 10) (channel/send out "sibling")))
              (channel/send out (llm/complete "root"))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("root completion and sibling settle");

    assert_eq!(strings(&value), ["sibling", "provider"]);
}

#[test]
#[serial]
fn root_retry_backoff_parks_while_a_sibling_runs() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .error(LlmError::RateLimited {
            retry_after_ms: 100,
        })
        .reply("provider")
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    set_network_max_retries(1);
    set_retry_base_ms(0);
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (let ((out (channel/new 2)))
              (async/spawn (fn () (sleep 10) (channel/send out "sibling")))
              (channel/send out (llm/complete "root"))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("root retry and sibling settle");

    assert_eq!(strings(&value), ["sibling", "provider"]);
}

#[test]
#[serial]
fn root_rate_limit_pacing_parks_while_a_sibling_runs() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .echo()
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (let ((out (channel/new 2)))
              (llm/with-rate-limit 10.0
                (fn ()
                  (llm/complete "first")
                  (async/spawn (fn () (sleep 10) (channel/send out "sibling")))
                  (channel/send out (llm/complete "provider"))))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("root pacing and sibling settle");

    assert_eq!(strings(&value), ["sibling", "provider"]);
}

#[test]
#[serial]
fn root_chat_keeps_a_sema_defined_provider_on_the_vm_thread() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (request)
                           (string-append "sema: "
                             (:content (first (:messages request)))))
               :default-model "sema-model"})
            (llm/chat (list (message :user "hello")))
            "#,
        )
        .expect("Sema-defined provider runs through the runtime root");

    assert_eq!(value.as_str(), Some("sema: hello"));
}

#[test]
#[serial]
fn root_batch_keeps_a_sema_defined_provider_on_the_vm_thread() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (request)
                           (:content (first (:messages request))))
               :default-model "sema-model"})
            (llm/batch (list "a" "b"))
            "#,
        )
        .expect("Sema-defined batch provider runs through the runtime root");

    assert_eq!(strings(&value), ["a", "b"]);
}

#[test]
#[serial]
fn root_image_extract_parks_while_a_sibling_runs() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(100)
        .reply(r#"{"description":"image"}"#)
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (let ((out (channel/new 2)))
              (async/spawn (fn () (sleep 10) (channel/send out "sibling")))
              (channel/send out
                (:description
                  (llm/extract-from-image
                    {:description :string}
                    (bytevector 137 80 78 71))))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("root image extraction and sibling settle");

    assert_eq!(strings(&value), ["sibling", "image"]);
}

#[test]
#[serial]
fn root_embed_parks_while_a_sibling_runs() {
    let fake = FakeProvider::builder("fake")
        .model("fake-embed")
        .embed_delay(100)
        .embed(vec![vec![0.1, 0.2, 0.3]])
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (let ((out (channel/new 2)))
              (async/spawn (fn () (sleep 10) (channel/send out "sibling")))
              (let ((embedding (llm/embed "root")))
                (channel/send out
                  (if (= (embedding/length embedding) 3) "embed" "wrong")))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("root embedding and sibling settle");

    assert_eq!(strings(&value), ["sibling", "embed"]);
}

#[test]
#[serial]
fn root_rerank_parks_while_a_sibling_runs() {
    let fake = FakeProvider::builder("fake")
        .model("fake-rerank")
        .rerank_delay(100)
        .rerank(&[(0, 0.9), (1, 0.1)])
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (let ((out (channel/new 2)))
              (async/spawn (fn () (sleep 10) (channel/send out "sibling")))
              (channel/send out
                (:document (first (llm/rerank "root" (list "rerank" "other")))))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("root rerank and sibling settle");

    assert_eq!(strings(&value), ["sibling", "rerank"]);
}

#[test]
#[serial]
fn root_batch_parks_while_a_sibling_runs() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(100)
        .reply("batch")
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (let ((out (channel/new 2)))
              (async/spawn (fn () (sleep 10) (channel/send out "sibling")))
              (channel/send out (first (llm/batch (list "root"))))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("root batch and sibling settle");

    assert_eq!(strings(&value), ["sibling", "batch"]);
}

#[test]
#[serial]
fn native_before_sema_fallback_is_rejected_before_native_dispatch() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("must-not-dispatch")
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let error = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (_request) "sema")
               :default-model "sema-model"})
            (llm/with-fallback [:fake :sema-provider]
              (fn () (llm/complete "root")))
            "#,
        )
        .expect_err("native-before-Sema fallback must be rejected before dispatch");

    assert!(
        error
            .to_string()
            .contains("place Sema-defined providers first"),
        "error must provide an immediately usable remedy: {error}"
    );
    assert_eq!(
        recorder.call_count(),
        0,
        "unsupported fallback ordering must not call the leading native provider"
    );
}

#[test]
#[serial]
fn sema_provider_rate_pacing_uses_a_structural_timer() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (request)
                           (:content (first (:messages request))))
               :default-model "sema-model"})
            (let ((out (channel/new 2)))
              (llm/with-rate-limit 10.0
                (fn ()
                  (llm/complete "first")
                  (async/spawn (fn () (sleep 10) (channel/send out "sibling")))
                  (channel/send out (llm/complete "provider"))))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("Sema-defined provider pacing and sibling settle");

    assert_eq!(strings(&value), ["sibling", "provider"]);
}

#[test]
#[serial]
fn cancelling_sema_provider_pacing_does_not_invoke_the_provider() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let started = Instant::now();
    let value = interp
        .eval_str_compiled(
            r#"
            (define calls 0)
            (llm/define-provider :sema-provider
              {:complete (fn (request)
                           (set! calls (+ calls 1))
                           (:content (first (:messages request))))
               :default-model "sema-model"})
            (llm/with-rate-limit 1.0
              (fn ()
                (llm/complete "issued")
                (define pending (async/spawn (fn () (llm/complete "cancelled"))))
                (async/spawn (fn () (sleep 20) (async/cancel pending)))
                (list (try (async/await pending) (catch error :cancelled)) calls)))
            "#,
        )
        .expect("Sema-defined provider pacing is cancellable");

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "cancellation must not wait out the one-second pacing timer"
    );
    let items = value.as_list().expect("cancel result and call count");
    assert_eq!(items[0], sema_core::Value::keyword("cancelled"));
    assert_eq!(items[1].as_int(), Some(1));
}

#[test]
#[serial]
fn sema_to_native_fallback_parks_before_the_native_provider() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(100)
        .reply("native")
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (request) (error "fall through"))
               :default-model "sema-model"})
            (let ((out (channel/new 2)))
              (llm/with-fallback [:sema-provider :fake]
                (fn ()
                  (async/spawn (fn () (sleep 10) (channel/send out "sibling")))
                  (channel/send out (llm/complete "root"))))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("mixed fallback reaches its native provider");

    assert_eq!(strings(&value), ["sibling", "native"]);
    assert_eq!(recorder.call_count(), 1);
}

#[test]
#[serial]
fn cancelling_retry_backoff_issues_no_retry_and_charges_nothing() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .error(LlmError::RateLimited {
            retry_after_ms: 1_000,
        })
        .reply("must-not-dispatch")
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    set_network_max_retries(1);
    set_retry_base_ms(0);
    register_test_provider(Box::new(fake));

    let started = Instant::now();
    let value = interp
        .eval_str_compiled(
            r#"
            (define pending (async/spawn (fn () (llm/complete "retry"))))
            (async/spawn (fn () (sleep 20) (async/cancel pending)))
            (list (try (async/await pending) (catch error :cancelled))
                  (:total-tokens (llm/session-usage)))
            "#,
        )
        .expect("retry wait is cancellable");

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "cancellation must not wait out the one-second retry delay"
    );
    let items = value.as_list().expect("cancel result and usage");
    assert_eq!(items[0], sema_core::Value::keyword("cancelled"));
    assert_eq!(items[1].as_int(), Some(0));
    assert_eq!(
        recorder.call_count(),
        1,
        "the cancelled retry must never issue its second request"
    );
}

#[test]
#[serial]
fn cancelling_rate_pacing_issues_no_request_or_usage_charge() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .echo()
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let started = Instant::now();
    let value = interp
        .eval_str_compiled(
            r#"
            (llm/with-rate-limit 1.0
              (fn ()
                (llm/complete "issued")
                (define pending (async/spawn (fn () (llm/complete "cancelled"))))
                (async/spawn (fn () (sleep 20) (async/cancel pending)))
                (list (try (async/await pending) (catch error :cancelled))
                      (:total-tokens (llm/session-usage)))))
            "#,
        )
        .expect("rate pacing wait is cancellable");

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "cancellation must not wait out the one-second pacing delay"
    );
    let items = value.as_list().expect("cancel result and usage");
    assert_eq!(items[0], sema_core::Value::keyword("cancelled"));
    assert_eq!(
        items[1].as_int(),
        Some(15),
        "only the first, issued request contributes usage"
    );
    assert_eq!(
        recorder.call_count(),
        1,
        "the cancelled paced call must not reach the provider"
    );
}
