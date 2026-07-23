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
fn concurrent_tasks_keep_fallback_and_last_usage_scopes_isolated() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :provider-a
              {:complete (fn (_request) "reply-a")
               :default-model "model-a"})
            (llm/define-provider :provider-b
              {:complete (fn (_request) "reply-b")
               :default-model "model-b"})
            (let ((out (channel/new 2)))
              (async/spawn
                (fn ()
                  (llm/with-fallback [:provider-a]
                    (fn ()
                      (sleep 10)
                      (define reply (llm/complete "task-a"))
                      (sleep 30)
                      (channel/send out
                        (list "task-a" reply (:model (llm/last-usage))))))))
              (async/spawn
                (fn ()
                  (llm/with-fallback [:provider-b]
                    (fn ()
                      (sleep 20)
                      (define reply (llm/complete "task-b"))
                      (channel/send out
                        (list "task-b" reply (:model (llm/last-usage))))))))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("concurrent dynamic scopes settle");

    let rows = value.as_list().expect("two task rows");
    let row = |label: &str| {
        rows.iter()
            .find_map(|value| {
                let values = value.as_list()?;
                (values.first()?.as_str() == Some(label)).then_some(values)
            })
            .unwrap_or_else(|| panic!("missing result for {label}"))
    };
    let task_a = row("task-a");
    let task_b = row("task-b");
    assert_eq!(task_a[1].as_str(), Some("reply-a"));
    assert_eq!(task_a[2].as_str(), Some("model-a"));
    assert_eq!(task_b[1].as_str(), Some("reply-b"));
    assert_eq!(task_b[2].as_str(), Some("model-b"));
}

#[test]
#[serial]
fn spawned_task_starts_without_its_parents_last_usage() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :provider-a
              {:complete (fn (_request) "parent-reply")
               :default-model "parent-model"})
            (llm/with-fallback [:provider-a]
              (fn ()
                (llm/complete "parent-call")
                (await (async/spawn (fn () (llm/last-usage))))))
            "#,
        )
        .expect("child reads its task-private last-usage slot");

    assert!(value.is_nil(), "child inherited parent usage: {value}");
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
fn native_before_sema_fallback_reaches_the_sema_provider() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .error(LlmError::Config("fall through".to_string()))
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (_request) "sema")
               :default-model "sema-model"})
            (llm/with-fallback [:fake :sema-provider]
              (fn () (llm/complete "root")))
            "#,
        )
        .expect("native-before-Sema fallback reaches its second provider");

    assert_eq!(value.as_str(), Some("sema"));
    assert_eq!(recorder.call_count(), 1);
}

#[test]
#[serial]
fn sema_provider_callback_parks_while_a_sibling_runs() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (_request) (sleep 100) "provider")
               :default-model "sema-model"})
            (let ((out (channel/new 2)))
              (async/spawn (fn () (sleep 10) (channel/send out "sibling")))
              (channel/send out (llm/complete "root"))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("Sema-defined provider callback and sibling settle");

    assert_eq!(strings(&value), ["sibling", "provider"]);
}

#[test]
#[serial]
fn sema_provider_callback_can_spawn_and_await_runtime_work() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (_request)
                           (define pending
                             (async/spawn (fn () (sleep 10) "nested")))
                           (string-append "provider-" (async/await pending)))
               :default-model "sema-model"})
            (llm/complete "root")
            "#,
        )
        .expect("Sema-defined provider callback can use runtime primitives");

    assert_eq!(value.as_str(), Some("provider-nested"));
}

#[test]
#[serial]
fn sema_provider_callback_observes_the_callers_dynamic_context() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (_request) (context/get :request-id))
               :default-model "sema-model"})
            (context/with {:request-id "req-42"}
              (fn () (llm/complete "root")))
            "#,
        )
        .expect("Sema-defined provider inherits its caller context");

    assert_eq!(value.as_str(), Some("req-42"));
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
fn cancelling_a_sema_provider_callback_does_not_fall_back_or_charge_usage() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("must-not-dispatch")
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let started = Instant::now();
    let value = interp
        .eval_str_compiled(
            r#"
            (llm/define-provider :sema-provider
              {:complete (fn (_request) (sleep 1000) "late")
               :default-model "sema-model"})
            (llm/with-fallback [:sema-provider :fake]
              (fn ()
                (define pending (async/spawn (fn () (llm/complete "cancelled"))))
                (async/spawn (fn () (sleep 20) (async/cancel pending)))
                (list (try (async/await pending) (catch error :cancelled))
                      (:total-tokens (llm/session-usage)))))
            "#,
        )
        .expect("Sema-defined provider callback is cancellable");

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "cancellation must not wait out the provider callback's sleep"
    );
    let items = value.as_list().expect("cancel result and usage");
    assert_eq!(items[0], sema_core::Value::keyword("cancelled"));
    assert_eq!(items[1].as_int(), Some(0));
    assert_eq!(
        recorder.call_count(),
        0,
        "cancellation must not advance into the fallback provider"
    );
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

/// A disk-cache HIT parks the root task on the offloaded disk read (C3), so a sibling
/// spawned beforehand runs to completion while the root is suspended. If the disk read
/// ran synchronously on the quantum the root would send its cached answer before ever
/// yielding, and the order would flip. Run 1 populates the on-disk cache; run 2 (fresh
/// interpreter, empty in-memory cache) can only be served from disk.
#[test]
#[serial]
fn disk_cache_hit_parks_while_a_sibling_runs() {
    let prompt = format!("disk-park-probe-{}", std::process::id());

    // Run 1: populate the on-disk cache with a real completion.
    let rec_fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("cached")
        .build();
    {
        let interp = Interpreter::new();
        reset_runtime_state();
        register_test_provider(Box::new(rec_fake));
        interp
            .eval_str_compiled(&format!(
                r#"(llm/with-cache {{:ttl 3600}} (fn () (llm/complete "{prompt}" {{:model "fake-chat"}})))"#
            ))
            .expect("run 1 populates the disk cache");
    }

    // Run 2: a fake that ERRORS if the provider is called — the hit is served from
    // disk. The instant sibling only lands "sibling" first if the root parked on the
    // offloaded peek.
    let must_not_call = FakeProvider::builder("fake")
        .model("fake-chat")
        .error(LlmError::Api {
            status: 500,
            message: "disk cache hit must not call the provider".into(),
        })
        .build();
    let recorder = must_not_call.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(must_not_call));
    let value = interp
        .eval_str_compiled(&format!(
            r#"
            (llm/with-cache {{:ttl 3600}}
              (fn ()
                (let ((out (channel/new 2)))
                  (async/spawn (fn () (channel/send out "sibling")))
                  (channel/send out (llm/complete "{prompt}" {{:model "fake-chat"}}))
                  (list (channel/recv out) (channel/recv out)))))
            "#
        ))
        .expect("disk cache hit settles with a runnable sibling");

    assert_eq!(
        strings(&value),
        ["sibling", "cached"],
        "the sibling runs while the root parks on the offloaded cache disk read"
    );
    assert_eq!(
        recorder.call_count(),
        0,
        "the disk cache hit must not call the provider"
    );
}

/// The cache and cassette disk legs MUST run OFF the runtime quantum (C3). This drives
/// a cache miss+store and a cassette record (tape load + save) through the cooperative
/// runtime and asserts the release-safe seam counter — every raw cache/cassette fs site
/// bumps it when it runs while a quantum is active — stays at 0.
#[test]
#[serial]
fn cache_and_cassette_do_no_filesystem_io_on_the_quantum() {
    use sema_llm::builtins::{quantum_fs_calls, reset_quantum_fs_calls};

    let prompt = format!("seam-probe-{}", std::process::id());
    let cassette = std::env::temp_dir().join(format!("sema-seam-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&cassette);

    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("a")
        .reply("b")
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));
    reset_quantum_fs_calls();

    interp
        .eval_str_compiled(&format!(
            r#"
            (llm/with-cache {{:ttl 3600}} (fn () (llm/complete "{prompt}" {{:model "fake-chat"}})))
            (llm/with-cassette "{cass}" {{:mode :record}}
              (fn () (llm/complete "{prompt}-cass" {{:model "fake-chat"}})))
            "#,
            cass = cassette.display()
        ))
        .expect("cache + cassette workload runs under the runtime");

    assert_eq!(
        quantum_fs_calls(),
        0,
        "cache/cassette filesystem I/O must never run on the runtime quantum"
    );
    let _ = std::fs::remove_file(&cassette);
}

/// Custom pricing is TASK-SNAPSHOT config (C4): it is parked onto a suspended task via
/// the LLM dynamic scope, so a sibling's `(llm/set-pricing ...)` cannot reprice work
/// already recorded by a parked task. Task A prices its model expensively and parks;
/// while parked, task B reprices the SAME model cheaply. When A resumes and reads its
/// last-usage cost, it MUST reflect A's own (expensive) snapshot — not B's change. On
/// the pre-C4 ambient-TLS pricing, A would read B's cheap price and this fails.
#[test]
#[serial]
fn sibling_custom_pricing_change_does_not_reprice_suspended_task() {
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
              (async/spawn
                (fn ()
                  (llm/set-pricing "fake-chat" 6000.0 12000.0)   ; A: expensive snapshot
                  (llm/complete "a")                             ; records A's usage
                  (sleep 40)                                     ; park while B reprices
                  (channel/send out (list "a" (:cost-usd (llm/last-usage))))))
              (async/spawn
                (fn ()
                  (sleep 15)
                  (llm/set-pricing "fake-chat" 1.0 2.0)          ; B: cheap — A must not see it
                  (llm/complete "b")
                  (channel/send out (list "b" (:cost-usd (llm/last-usage))))))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("custom pricing stays task-private across suspension");

    let rows = value.as_list().expect("two task rows");
    let row = |label: &str| {
        rows.iter()
            .find_map(|value| {
                let values = value.as_list()?;
                (values.first()?.as_str() == Some(label)).then_some(values)
            })
            .unwrap_or_else(|| panic!("missing result for {label}"))
    };
    // usage is (prompt=10, completion=5); A: (10*6000 + 5*12000)/1e6 = 0.12,
    // B: (10*1 + 5*2)/1e6 = 0.00002.
    let cost = |values: &[sema_core::Value]| values[1].as_float().expect("cost float");
    assert!(
        (cost(row("a")) - 0.12).abs() < 1e-9,
        "task A must price with its OWN expensive snapshot; got {}",
        cost(row("a"))
    );
    assert!(
        (cost(row("b")) - 0.00002).abs() < 1e-9,
        "task B prices with its own cheap snapshot; got {}",
        cost(row("b"))
    );
}

/// Nested `llm/with-budget` scopes keep their save-stack TASK-PRIVATE (C4). Two tasks
/// each open an outer then an inner budget and park inside the inner while the sibling
/// does the same, so their pushes interleave. When each inner scope tears down it must
/// restore that task's OWN outer frame. On the pre-C4 ambient `BUDGET_STACK` the pops
/// cross out of LIFO order and a task restores the wrong (or no) frame.
#[test]
#[serial]
fn interleaved_nested_budget_scopes_restore_their_own_frames() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (let ((out (channel/new 2)))
              (async/spawn
                (fn ()
                  (llm/with-budget {:max-cost-usd 1.0}
                    (fn ()
                      (sleep 10)                                   ; let B enter its outer
                      (llm/with-budget {:max-cost-usd 2.0}
                        (fn () (sleep 40)))                        ; park in inner while B nests
                      ;; inner popped: active frame must be A's OWN outer (1.0)
                      (channel/send out (list "a" (:limit (llm/budget-remaining))))))))
              (async/spawn
                (fn ()
                  (llm/with-budget {:max-cost-usd 10.0}
                    (fn ()
                      (sleep 20)                                   ; enter after A's outer
                      (llm/with-budget {:max-cost-usd 20.0}
                        (fn () (sleep 10)))                        ; B's inner pops while A parks
                      (channel/send out (list "b" (:limit (llm/budget-remaining))))))))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("nested budget scopes settle");

    let rows = value.as_list().expect("two task rows");
    let row = |label: &str| {
        rows.iter()
            .find_map(|value| {
                let values = value.as_list()?;
                (values.first()?.as_str() == Some(label)).then_some(values)
            })
            .unwrap_or_else(|| panic!("missing result for {label}"))
    };
    let limit = |values: &[sema_core::Value]| values[1].as_float().expect("budget limit float");
    assert!(
        (limit(row("a")) - 1.0).abs() < 1e-9,
        "task A's inner teardown must restore A's outer budget (1.0); got {}",
        limit(row("a"))
    );
    assert!(
        (limit(row("b")) - 10.0).abs() < 1e-9,
        "task B's inner teardown must restore B's outer budget (10.0); got {}",
        limit(row("b"))
    );
}
