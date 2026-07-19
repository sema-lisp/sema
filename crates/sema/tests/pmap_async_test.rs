//! Cooperative mapper coverage for `llm/pmap`.
//!
//! Prompt construction runs as ordinary Sema work on the active runtime task;
//! only after every mapper call succeeds does `llm/batch` dispatch provider
//! requests. These tests use the deterministic `FakeProvider`, so suspension,
//! cancellation, request shaping, and provider-call counts need no API keys.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use sema_core::Value;
use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::{FakeProvider, FakeRecorder};

fn eval_with_fake(
    src: &str,
    fake: FakeProvider,
) -> (Result<Value, sema_core::SemaError>, Arc<FakeRecorder>) {
    let interp = Interpreter::new();
    reset_runtime_state();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    let result = interp.eval_str_compiled(src);
    (result, recorder)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_list()
        .expect("expected list")
        .iter()
        .map(|item| item.as_str().expect("expected string").to_string())
        .collect()
}

#[test]
fn pmap_mapper_suspends_while_sibling_runs() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("mapped reply")
        .build();
    let (result, recorder) = eval_with_fake(
        r#"
        (let ((out (channel/new 2)))
          (let ((mapped
                  (async/spawn
                    (fn ()
                      (channel/send out
                        (first
                          (llm/pmap
                            (fn (item)
                              (async/sleep 100)
                              (str "prompt-" item))
                            (list 1)
                            {:model "fake-chat"}))))))
                (sibling
                  (async/spawn
                    (fn ()
                      (async/sleep 10)
                      (channel/send out "sibling")))))
            (let ((received (list (channel/recv out) (channel/recv out))))
              (async/await mapped)
              (async/await sibling)
              received)))
        "#,
        fake,
    );

    let received = strings(&result.expect("suspending pmap should settle"));
    assert_eq!(received, vec!["sibling", "mapped reply"]);
    assert_eq!(recorder.call_count(), 1);
}

#[test]
fn pmap_mapper_failure_prevents_provider_dispatch() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("must not dispatch")
        .build();
    let (result, recorder) = eval_with_fake(
        r#"
        (let ((mapped 0))
          (try
            (llm/pmap
              (fn (item)
                (set! mapped (+ mapped 1))
                (if (= item 2) (error "mapper boom") (str "prompt-" item)))
              (list 1 2 3)
              {:model "fake-chat"})
            (catch error mapped)))
        "#,
        fake,
    );

    assert_eq!(
        result.expect("mapper failure should be catchable").as_int(),
        Some(2)
    );
    assert_eq!(
        recorder.call_count(),
        0,
        "no provider request may start until every mapper call succeeds"
    );
}

#[test]
fn cancelling_pmap_stops_remaining_mapping_and_provider_dispatch() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("must not dispatch")
        .build();
    let (result, recorder) = eval_with_fake(
        r#"
        (let ((mapped 0))
          (let ((pending
                  (async/spawn
                    (fn ()
                      (llm/pmap
                        (fn (item)
                          (set! mapped (+ mapped 1))
                          (async/sleep 1000)
                          (str "prompt-" item))
                        (list 1 2 3)
                        {:model "fake-chat"})))))
            (let ((cancelled
                    (async/await
                      (async/spawn
                        (fn ()
                          (async/sleep 20)
                          (async/cancel pending))))))
              (try (async/await pending) (catch error nil))
              (list cancelled (async/cancelled? pending) mapped))))
        "#,
        fake,
    );

    let values = result
        .expect("pmap cancellation program should settle")
        .as_list()
        .expect("cancellation result list")
        .to_vec();
    assert_eq!(
        values,
        vec![Value::bool(true), Value::bool(true), Value::int(1)]
    );
    assert_eq!(
        recorder.call_count(),
        0,
        "cancelling during mapping must not launch the batch"
    );
}

#[test]
fn pmap_mapper_preserves_captured_mutation_and_task_context_across_suspension() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("r0")
        .reply("r1")
        .build();
    let (result, recorder) = eval_with_fake(
        r#"
        (context/with {:prefix "ctx"}
          (fn ()
            (let ((mapped 0))
              (llm/pmap
                (fn (item)
                  (set! mapped (+ mapped 1))
                  (async/sleep 5)
                  (format "~a-~a-~a" (context/get :prefix) item mapped))
                [1 2]
                {:model "fake-chat"}))))
        "#,
        fake,
    );

    assert_eq!(
        strings(&result.expect("contextual pmap should settle")),
        vec!["r0", "r1"]
    );
    let requests = recorder.requests();
    let prompts: Vec<String> = requests
        .iter()
        .map(|request| request.messages[0].content.to_text())
        .collect();
    assert_eq!(prompts, vec!["ctx-1-1", "ctx-2-2"]);
}

#[test]
fn pmap_preserves_public_shape_prompt_stringification_and_options() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("r0")
        .reply("r1")
        .build();
    let (result, recorder) = eval_with_fake(
        r#"
        (let ((pmap-value llm/pmap))
          (list
            (procedure? pmap-value)
            (pmap-value
              (fn (item) (if (= item 1) :alpha 2))
              [1 2]
              {:model "fake-chat"
               :max-tokens 123
               :temperature 0.25
               :system "system prompt"})))
        "#,
        fake,
    );

    let value = result.expect("top-level pmap should succeed");
    let fields = value.as_list().expect("public shape result");
    assert_eq!(fields[0], Value::bool(true));
    assert_eq!(strings(&fields[1]), vec!["r0", "r1"]);

    let requests = recorder.requests();
    assert_eq!(requests.len(), 2);
    let prompts: Vec<String> = requests
        .iter()
        .map(|request| request.messages[0].content.to_text())
        .collect();
    assert_eq!(prompts, vec![":alpha", "2"]);
    for request in &requests {
        assert_eq!(request.model, "fake-chat");
        assert_eq!(request.max_tokens, Some(123));
        assert_eq!(request.temperature, Some(0.25));
        assert_eq!(request.system.as_deref(), Some("system prompt"));
    }
}

#[test]
fn pmap_preserves_synchronous_host_callback_behavior() {
    let interp = Interpreter::new();
    reset_runtime_state();
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("sync-r0")
        .reply("sync-r1")
        .build();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));

    let mapper = interp
        .eval_str_compiled(r#"(fn (item) (str "sync-" item))"#)
        .expect("compile mapper closure");
    let pmap = interp
        .global_env
        .get(sema_core::intern("llm/pmap"))
        .expect("public pmap binding");
    let value = sema_core::call_callback(
        &interp.ctx,
        &pmap,
        &[
            mapper,
            Value::vector(vec![Value::int(1), Value::int(2)]),
            Value::map(std::collections::BTreeMap::from([(
                Value::keyword("model"),
                Value::string("fake-chat"),
            )])),
        ],
    )
    .expect("host callback should use synchronous map and batch ABIs");

    assert_eq!(strings(&value), vec!["sync-r0", "sync-r1"]);
    assert_eq!(recorder.call_count(), 2);
    let prompts: Vec<String> = recorder
        .requests()
        .iter()
        .map(|request| request.messages[0].content.to_text())
        .collect();
    assert_eq!(prompts, vec!["sync-1", "sync-2"]);
}

#[test]
fn pmap_preserves_exact_arity_errors() {
    let fake = FakeProvider::builder("fake").model("fake-chat").build();
    let (zero_args, recorder) = eval_with_fake("(llm/pmap)", fake);
    assert!(zero_args
        .expect_err("zero-argument pmap must fail")
        .to_string()
        .contains("llm/pmap expects 2-3 args, got 0"));
    assert_eq!(recorder.call_count(), 0);

    let fake = FakeProvider::builder("fake").model("fake-chat").build();
    let (four_args, recorder) = eval_with_fake("(llm/pmap + (list) {} {})", fake);
    assert!(four_args
        .expect_err("four-argument pmap must fail")
        .to_string()
        .contains("llm/pmap expects 2-3 args, got 4"));
    assert_eq!(recorder.call_count(), 0);
}

#[test]
fn pmap_preserves_list_or_vector_collection_contract() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("must not dispatch")
        .build();
    let (result, recorder) = eval_with_fake(
        r#"(llm/pmap str (mutable-array/new 2 :item) {:model "fake-chat"})"#,
        fake,
    );

    let error = result.expect_err("pmap must continue to reject mutable arrays");
    assert!(
        error.to_string().contains("expected list or vector"),
        "unexpected collection error: {error}"
    );
    assert_eq!(recorder.call_count(), 0);
}
