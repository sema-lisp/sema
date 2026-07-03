//! Gate for concurrent single-shot `llm/complete` · `llm/classify` · `llm/extract`
//! with full per-task OTel tracing — the chat counterpart of
//! `embed_async_otel_test.rs`.
//!
//! When run as scheduler tasks (`async/pool-map`, `async/all` over `async/spawn`),
//! these completions must (1) OVERLAP their network round-trips on the cooperative
//! scheduler (wall ≈ max, not sum; peak in-flight ≥ 2), (2) each emit a DISTINCT,
//! correctly-isolated `chat` span, and (3) account `track_usage` EXACTLY ONCE per
//! completion (no double-charge). The synchronous (top-level) path stays
//! byte-identical. Cache-hit and cassette-replay in an async context return WITHOUT
//! yielding and report zero usage on a cache hit.
//!
//! Deterministic + keyless (a delayed FakeProvider chat reply). Own binary — the
//! in-memory exporter, `sema_otel::testing::install()`, and the `IO_INFLIGHT`
//! instrumentation atomics are process-global, so these `#[serial]` tests must not
//! share a process with unrelated span/inflight capture.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{
    io_peak_inflight, register_test_provider, reset_io_inflight, reset_runtime_state,
};
use sema_llm::fake::FakeProvider;
use serial_test::serial;

/// `(async/pool-map llm/complete prompts 3)` over a delayed fake: results correct
/// and in INPUT order, with overlap proven by both peak in-flight ≥ 2 and a
/// max-not-sum wall clock.
#[test]
#[serial]
fn pool_map_complete_overlaps_and_preserves_order() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    // Echo mode: each reply is the prompt text itself, so reply↔prompt correlation
    // is deterministic regardless of which worker finishes first — the clean way to
    // prove pool-map preserves INPUT order under out-of-order completion. 300 ms
    // delay each.
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(300)
        .echo()
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // pool size 3 ⇒ all three run together: serial floor ~900 ms, overlapped ~300 ms.
    let program = r#"
        (let ((t0 (sys/elapsed)))
          (let ((res (async/pool-map llm/complete (list "p0" "p1" "p2") 3)))
            (list res (floor (/ (- (sys/elapsed) t0) 1000000)))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("async/pool-map llm/complete evaluated");
    let outer = result.as_list().expect("(results wall-ms)");
    let res = outer[0].as_list().expect("results list");
    assert_eq!(res.len(), 3, "three completions");
    // Input order preserved (pool-map returns in input order, even if the workers
    // completed out of order).
    assert_eq!(res[0].as_str(), Some("p0"));
    assert_eq!(res[1].as_str(), Some("p1"));
    assert_eq!(res[2].as_str(), Some("p2"));

    let wall_ms = outer[1].as_int().expect("wall ms");
    assert!(
        wall_ms < 700,
        "expected overlapped wall < 700 ms (serial floor ~900 ms), got {wall_ms} ms"
    );
    assert!(
        io_peak_inflight() >= 2,
        "expected peak in-flight >= 2 (true overlap), got {}",
        io_peak_inflight()
    );
}

/// ASYNC-1 (Scope A): a completion in a task spawned INSIDE `llm/with-cache` but
/// AWAITED OUTSIDE the thunk's dynamic extent must still participate in the cache.
/// The per-task dynamic-scope capture carries `CACHE_ENABLED` onto the task (seeded
/// at `async/spawn`), so the deferred completion counts a miss and a same-prompt
/// repeat is served as a hit — where before the task read the already-reset flag and
/// silently bypassed the cache (`:misses 0`). This is the gate removed as flaky.
#[test]
#[serial]
fn async_cache_miss_is_counted() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(30)
        .echo()
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // `llm/cache-clear` wipes the persistent (on-disk) cache too — `reset_runtime_state`
    // only clears the in-memory table, so a "hello" entry left by a prior run would make
    // this dispatch a HIT and the intended first-miss would never be counted (the source
    // of the earlier "flaky" removal — it was actually a non-hermetic disk cache).
    // Spawn is inside the with-cache thunk; the await is OUTSIDE it, so the task
    // executes after the with-cache extent has ended.
    let program = r#"
        (llm/cache-clear)
        (define p (llm/with-cache (fn () (async/spawn (fn () (llm/complete "hello"))))))
        (async/await p)
        (:misses (llm/cache-stats))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("async cache program evaluated");
    assert_eq!(
        result.as_int(),
        Some(1),
        "a deferred async completion inside with-cache must count a cache miss \
         (task inherits the with-cache dynamic scope)"
    );
}

/// ASYNC-1 (Scope B, the correctness fix): a `with-budget` cap must gate the AGGREGATE
/// of a CONCURRENT fan-out. Three completions spawned inside `with-budget` and awaited
/// outside each cost $1.5 (echo usage 10+5 tokens at the priced rate). The $4.0 cap is
/// chosen to sit ABOVE any single or double call ($1.5, $3.0) but BELOW the full
/// aggregate ($4.5) — so it can only trip once ALL THREE have charged the shared frame,
/// which both proves aggregate gating and avoids stranding an in-flight sibling (the
/// last-charging task is the one that fails; the other two are already Done). Before
/// the fix every deferred task charged whatever budget frame was installed when its
/// future landed (None, popped after the thunk returned), so the fan-out silently
/// completed uncapped. The fix makes the active budget a SHARED frame captured by-`Rc`
/// onto each task, so all siblings charge one aggregate.
#[test]
#[serial]
fn async_budget_gates_concurrent_fanout() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(30)
        .echo()
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // Price so each echo completion (10 in + 5 out) costs $1.5; only the full trio
    // ($4.5) exceeds the $4.0 cap. Spawn inside with-budget, await outside.
    let program = r#"
        (llm/set-pricing "fake-chat" 100000.0 100000.0)
        (define ps
          (llm/with-budget {:max-cost-usd 4.0}
            (fn ()
              (list (async/spawn (fn () (llm/complete "a")))
                    (async/spawn (fn () (llm/complete "b")))
                    (async/spawn (fn () (llm/complete "c")))))))
        (async/all ps)
    "#;
    let err = interp
        .eval_str_compiled(program)
        .expect_err("concurrent fan-out aggregate must exceed the shared budget");
    assert!(
        err.to_string().contains("budget exceeded"),
        "expected a budget-exceeded error from the concurrent fan-out, got: {err}"
    );
}

/// `llm/classify` batched over `async/pool-map` overlaps and returns the correct
/// categories (as keywords, matching the keyword category list).
#[test]
#[serial]
fn classify_batch_overlaps_and_returns_categories() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    // Echo mode: the classification reply is the prompt text, so passing the
    // category name as the text yields that category — deterministic regardless of
    // worker completion order (proving input-order preservation under overlap).
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(300)
        .echo()
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (let ((t0 (sys/elapsed)))
          (let ((res (async/pool-map
                       (fn (t) (llm/classify (list :positive :negative) t))
                       (list "positive" "negative" "positive") 3)))
            (list res (floor (/ (- (sys/elapsed) t0) 1000000)))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("async/pool-map llm/classify evaluated");
    let outer = result.as_list().expect("(results wall-ms)");
    let res = outer[0].as_list().expect("results list");
    assert_eq!(res.len(), 3);
    assert_eq!(res[0].as_keyword().as_deref(), Some("positive"));
    assert_eq!(res[1].as_keyword().as_deref(), Some("negative"));
    assert_eq!(res[2].as_keyword().as_deref(), Some("positive"));

    let wall_ms = outer[1].as_int().expect("wall ms");
    assert!(
        wall_ms < 700,
        "expected overlapped wall < 700 ms, got {wall_ms} ms"
    );
    assert!(
        io_peak_inflight() >= 2,
        "expected peak in-flight >= 2, got {}",
        io_peak_inflight()
    );
}

/// N concurrent completes produce N distinct, non-cross-contaminated `chat` spans:
/// distinct trace_ids + span_ids, neither parenting the other, each carrying its
/// OWN input-token total (the per-task otel-isolation proof).
#[test]
#[serial]
fn concurrent_completes_emit_isolated_chat_spans() {
    let cap = sema_otel::testing::install();
    reset_io_inflight();

    // Two completions in spawn order with DISTINCT prompt-token counts (7 / 11).
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(200)
        .reply_with_usage("first", 7, 3)
        .reply_with_usage("second", 11, 5)
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (async/all
          (map (fn (p) (async/spawn (fn () (llm/complete p))))
               (list "alpha" "beta")))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("concurrent completes evaluated");
    let res = result.as_list().expect("results list");
    assert_eq!(res.len(), 2);

    let spans = cap.spans_json();
    let chat_spans: Vec<&serde_json::Value> = spans
        .iter()
        .filter(|s| s["attributes"]["gen_ai.operation.name"] == "chat")
        .collect();
    assert_eq!(
        chat_spans.len(),
        2,
        "expected exactly two chat spans, got {}",
        chat_spans.len()
    );

    let trace_ids: Vec<&serde_json::Value> = chat_spans.iter().map(|s| &s["trace_id"]).collect();
    let span_ids: Vec<&serde_json::Value> = chat_spans.iter().map(|s| &s["span_id"]).collect();
    assert_ne!(span_ids[0], span_ids[1], "distinct span_ids");
    assert_ne!(
        trace_ids[0], trace_ids[1],
        "each spawned complete is its own root trace"
    );
    for s in &chat_spans {
        let parent = &s["parent_span_id"];
        assert_ne!(
            parent, span_ids[0],
            "span parented under the other complete"
        );
        assert_ne!(
            parent, span_ids[1],
            "span parented under the other complete"
        );
    }

    // Each span carries its own input-token total (7 and 11): proves the per-task
    // otel context swap kept the two detached spans isolated.
    let mut tokens: Vec<i64> = chat_spans
        .iter()
        .map(|s| {
            s["attributes"]["gen_ai.usage.input_tokens"]
                .as_i64()
                .expect("input_tokens present")
        })
        .collect();
    tokens.sort_unstable();
    assert_eq!(tokens, vec![7, 11], "each span its own input tokens");
    for s in &chat_spans {
        assert_eq!(s["kind"], "client");
        assert_eq!(s["attributes"]["gen_ai.provider.name"], "fake");
    }
}

/// Accounting: `track_usage` runs EXACTLY ONCE per completion (no double-charge).
/// Two concurrent completes of 100+50 tokens each ⇒ session total = 300, not 600.
#[test]
#[serial]
fn concurrent_completes_account_exactly_once() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(100)
        .reply_with_usage("a", 100, 50)
        .reply_with_usage("b", 100, 50)
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (async/all
          (map (fn (p) (async/spawn (fn () (llm/complete p))))
               (list "x" "y")))
        (:total-tokens (llm/session-usage))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("concurrent completes accounted");
    assert_eq!(
        val.as_int(),
        Some(300),
        "two completions of 150 tokens each must total 300 (accounted exactly once)"
    );
}

/// Cache hit in an async context returns WITHOUT yielding and reports ZERO usage
/// (the zero-usage cache-hit accounting invariant holds on the concurrent path).
#[test]
#[serial]
fn async_cache_hit_returns_zero_usage_without_yield() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    // ONE scripted reply: the 2nd concurrent call must be served from cache, or the
    // fake would error ("no scripted reply left").
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply_with_usage("cached!", 100, 50)
        .build();
    let recorder = fake.recorder();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // Prime the cache with one sync call, then fire two concurrent identical calls:
    // both hit the cache. Total session tokens = 150 (the one priming call only).
    let program = r#"
        (llm/cache-clear)
        (llm/with-cache {:ttl 3600}
          (fn ()
            (llm/complete "same")
            (async/all
              (map (fn (_) (async/spawn (fn () (llm/complete "same"))))
                   (list 1 2)))))
        (:total-tokens (llm/session-usage))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("async cache-hit run should succeed");
    assert_eq!(
        val.as_int(),
        Some(150),
        "cache hits must add 0 usage; only the priming call counts"
    );
    assert_eq!(
        recorder.call_count(),
        1,
        "only the priming call hits the provider; concurrent calls served from cache"
    );
}

/// Cassette replay in an async context returns the recorded content WITHOUT calling
/// the provider (and without a real round-trip yield path that touches the network).
#[test]
#[serial]
fn async_cassette_replay_serves_recorded_without_provider() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();
    let path = std::env::temp_dir().join(format!(
        "sema-cassette-async-{}-replay.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    // Record one interaction synchronously.
    let rec = FakeProvider::builder("fake")
        .model("m")
        .reply("taped")
        .build();
    {
        let interp = Interpreter::new();
        reset_runtime_state();
        register_test_provider(Box::new(rec));
        interp
            .eval_str_compiled(&format!(
                r#"(llm/with-cassette "{}" {{:mode :record}}
                     (fn () (llm/complete "the prompt" {{:model "m"}})))"#,
                path.display()
            ))
            .expect("record run should succeed");
    }

    // Replay concurrently: a fake that ERRORS if actually called. Getting "taped"
    // back from a spawned task proves the cassette served it on the async path.
    let replay_fake = FakeProvider::builder("fake")
        .model("m")
        .error(sema_llm::types::LlmError::Api {
            status: 500,
            message: "provider must not be called on replay".into(),
        })
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(replay_fake));
    let val = interp
        .eval_str_compiled(&format!(
            r#"(llm/with-cassette "{}" {{:mode :replay}}
                 (fn ()
                   (first (async/all
                            (list (async/spawn
                                    (fn () (llm/complete "the prompt" {{:model "m"}}))))))))"#,
            path.display()
        ))
        .expect("async replay run should succeed without touching the provider");
    assert_eq!(val.as_str(), Some("taped"));

    let _ = std::fs::remove_file(&path);
}

/// `llm/extract` (single-attempt, validation off) is offloaded and overlaps when
/// run as concurrent tasks, returning the parsed JSON per task.
#[test]
#[serial]
fn extract_single_attempt_overlaps() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(300)
        .reply(r#"{"name":"Ada"}"#)
        .reply(r#"{"name":"Bob"}"#)
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // validate off ⇒ single attempt each ⇒ both fully offloaded and overlapping.
    let program = r#"
        (let ((t0 (sys/elapsed)))
          (let ((res (async/all
                       (map (fn (t)
                              (async/spawn
                                (fn () (:name (llm/extract {:name "string"} t {:validate false})))))
                            (list "ada bio" "bob bio")))))
            (list res (floor (/ (- (sys/elapsed) t0) 1000000)))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("concurrent extract evaluated");
    let outer = result.as_list().expect("(results wall-ms)");
    let res = outer[0].as_list().expect("results list");
    assert_eq!(res.len(), 2);
    // Spawn order preserved by async/all; replies pop in worker-arrival order, so
    // assert the SET of names rather than positional order (single-attempt extract
    // has no per-prompt correlation knob here).
    let mut names: Vec<String> = res
        .iter()
        .map(|v| v.as_str().expect("name string").to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["Ada".to_string(), "Bob".to_string()]);

    let wall_ms = outer[1].as_int().expect("wall ms");
    assert!(
        wall_ms < 600,
        "expected overlapped wall < 600 ms (serial floor ~600 ms), got {wall_ms} ms"
    );
    assert!(
        io_peak_inflight() >= 2,
        "expected peak in-flight >= 2, got {}",
        io_peak_inflight()
    );
}

/// Sync path unchanged: a plain top-level `(llm/complete ...)` still works and
/// returns the content (no async context, no yield).
#[test]
#[serial]
fn sync_complete_outside_async_still_works() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("sync answer")
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let val = interp
        .eval_str_compiled(r#"(llm/complete "hello")"#)
        .expect("sync complete should run against the fake");
    assert_eq!(val.as_str(), Some("sync answer"));
}
