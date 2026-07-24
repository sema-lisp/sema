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

use std::time::{Duration, Instant};

use sema_eval::Interpreter;
use sema_llm::builtins::{
    install_cassette, io_peak_inflight, register_test_provider, reset_io_inflight,
    reset_runtime_state, take_cassette,
};
use sema_llm::cassette::{Cassette, CassetteMode};
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

/// Cassette replay follows the task spawned inside `llm/with-cassette` even when the
/// task is awaited only after the dynamic extent has unwound. The provider errors if
/// touched, so returning the taped response proves cassette selection belongs to the
/// captured task scope rather than whichever cassette is ambient when the task runs.
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

    // Record from a task that outlives the `with-cassette` body. The tape must be
    // retained by that task and flushed when its final scope owner is dropped.
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
                r#"(define pending
                     (llm/with-cassette "{}" {{:mode :record}}
                       (fn ()
                         (async/spawn
                           (fn () (llm/complete "the prompt" {{:model "m"}}))))))
                   (async/await pending)"#,
                path.display()
            ))
            .expect("deferred record task should retain and flush its cassette scope");
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
            r#"(define pending
                 (llm/with-cassette "{}" {{:mode :replay}}
                   (fn ()
                     (async/spawn
                       (fn () (llm/complete "the prompt" {{:model "m"}}))))))
               (async/await pending)"#,
            path.display()
        ))
        .expect("deferred replay task should retain its cassette scope");
    assert_eq!(val.as_str(), Some("taped"));

    let _ = std::fs::remove_file(&path);
}

/// Deferred embeddings retain the exact cassette selected at dispatch. Both the
/// record and replay tasks are awaited only after `llm/with-cassette` restores the
/// ambient scope; the replay provider has no embedding script, so any scope leak is
/// a hard error.
#[test]
#[serial]
fn async_embed_cassette_outlives_dynamic_scope() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();
    let path = std::env::temp_dir().join(format!(
        "sema-cassette-async-embed-{}-{}.jsonl",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&path);

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(
        FakeProvider::builder("fake")
            .model("m")
            .embed_delay(25)
            .embed(vec![vec![0.1, 0.2, 0.3]])
            .build(),
    ));
    let recorded = interp
        .eval_str_compiled(&format!(
            r#"(define pending
                 (llm/with-cassette "{}" {{:mode :record}}
                   (fn ()
                     (async/spawn
                       (fn () (llm/embed "deferred text" {{:model "m"}}))))))
               (async/await pending)"#,
            path.display()
        ))
        .expect("deferred embedding should record after its dynamic scope unwinds");
    assert_eq!(recorded.type_name(), "bytevector");

    reset_runtime_state();
    register_test_provider(Box::new(FakeProvider::builder("fake").model("m").build()));
    let replayed = interp
        .eval_str_compiled(&format!(
            r#"(define pending
                 (llm/with-cassette "{}" {{:mode :replay}}
                   (fn ()
                     (async/spawn
                       (fn () (llm/embed "deferred text" {{:model "m"}}))))))
               (async/await pending)"#,
            path.display()
        ))
        .expect("deferred embedding replay should not reach the provider");
    assert_eq!(replayed.type_name(), "bytevector");

    let _ = std::fs::remove_file(path);
}

/// A streaming run keeps its dispatch-time cassette through every parked chunk
/// wait and through finalization. The callback output proves recorded chunks are
/// replayed after the dynamic cassette extent has already unwound.
#[test]
#[serial]
fn async_stream_cassette_outlives_dynamic_scope() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();
    let path = std::env::temp_dir().join(format!(
        "sema-cassette-async-stream-{}-{}.jsonl",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&path);

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(
        FakeProvider::builder("fake")
            .model("m")
            .stream(&["deferred ", "stream"])
            .stream_chunk_delay(25)
            .build(),
    ));
    let recorded = interp
        .eval_str_compiled(&format!(
            r#"(define out "")
               (define pending
                 (llm/with-cassette "{}" {{:mode :record}}
                   (fn ()
                     (async/spawn
                       (fn ()
                         (llm/stream "deferred prompt"
                           (fn (chunk) (set! out (string-append out chunk)))
                           {{:model "m"}}))))))
               (async/await pending)
               out"#,
            path.display()
        ))
        .expect("deferred stream should record after its dynamic scope unwinds");
    assert_eq!(recorded.as_str(), Some("deferred stream"));

    reset_runtime_state();
    register_test_provider(Box::new(
        FakeProvider::builder("fake")
            .model("m")
            .error(sema_llm::types::LlmError::Api {
                status: 500,
                message: "provider must not be called on stream replay".into(),
            })
            .build(),
    ));
    let replayed = interp
        .eval_str_compiled(&format!(
            r#"(set! out "")
               (define pending
                 (llm/with-cassette "{}" {{:mode :replay}}
                   (fn ()
                     (async/spawn
                       (fn ()
                         (llm/stream "deferred prompt"
                           (fn (chunk) (set! out (string-append out chunk)))
                           {{:model "m"}}))))))
               (async/await pending)
               out"#,
            path.display()
        ))
        .expect("deferred stream replay should not reach the provider");
    assert_eq!(replayed.as_str(), Some("deferred stream"));

    let _ = std::fs::remove_file(path);
}

/// Ejecting the ambient cassette cannot detach it from tasks that already captured
/// the scope. One sibling is cancelled before dispatch; the surviving task records
/// after ejection, and reaping both tasks drops the final shared owner and flushes
/// the surviving interaction.
#[test]
#[serial]
fn cassette_eject_and_cancel_flush_on_last_task_owner() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();
    let path = std::env::temp_dir().join(format!(
        "sema-cassette-eject-cancel-{}-{}.jsonl",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&path);

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(
        FakeProvider::builder("fake").model("m").echo().build(),
    ));
    install_cassette(Cassette::load(path.clone(), CassetteMode::Record));
    interp
        .eval_str_compiled(
            r#"(define kept
                 (async/spawn
                   (fn ()
                     (async/sleep 25)
                     (llm/complete "kept prompt" {:model "m"}))))
               (define cancelled
                 (async/spawn
                   (fn ()
                     (async/sleep 500)
                     (llm/complete "cancelled prompt" {:model "m"}))))"#,
        )
        .expect("spawn two tasks under the installed cassette");

    drop(take_cassette().expect("eject the ambient cassette snapshot"));
    let kept = interp
        .eval_str_compiled(
            r#"(async/cancel cancelled)
               (define kept-value (async/await kept))
               (try (async/await cancelled) (catch error nil))
               kept-value"#,
        )
        .expect("cancel one captured task and reap both");
    assert_eq!(kept.as_str(), Some("kept prompt"));

    reset_runtime_state();
    register_test_provider(Box::new(
        FakeProvider::builder("fake")
            .model("m")
            .error(sema_llm::types::LlmError::Api {
                status: 500,
                message: "provider must not be called after last-owner flush".into(),
            })
            .build(),
    ));
    install_cassette(Cassette::load(path.clone(), CassetteMode::Replay));
    let replayed = interp
        .eval_str_compiled(r#"(llm/complete "kept prompt" {:model "m"})"#)
        .expect("last task owner should have flushed the surviving interaction");
    assert_eq!(replayed.as_str(), Some("kept prompt"));
    drop(take_cassette());

    let _ = std::fs::remove_file(path);
}

/// Sibling tasks spawned under different cassette extents retain distinct tape
/// identities after both extents unwind. A provider call is a hard failure, so the
/// ordered pair also proves one sibling cannot read the other's ambient cassette.
#[test]
#[serial]
fn async_cassette_scopes_are_isolated_between_siblings() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();
    let base = format!("sema-cassette-siblings-{}", std::process::id());
    let path_a = std::env::temp_dir().join(format!("{base}-a.jsonl"));
    let path_b = std::env::temp_dir().join(format!("{base}-b.jsonl"));
    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);

    let recorder = FakeProvider::builder("fake")
        .model("m")
        .reply("alpha")
        .reply("beta")
        .build();
    {
        let interp = Interpreter::new();
        reset_runtime_state();
        register_test_provider(Box::new(recorder));
        interp
            .eval_str_compiled(&format!(
                r#"(llm/with-cassette "{}" {{:mode :record}}
                     (fn () (llm/complete "prompt-a" {{:model "m"}})))
                   (llm/with-cassette "{}" {{:mode :record}}
                     (fn () (llm/complete "prompt-b" {{:model "m"}})))"#,
                path_a.display(),
                path_b.display()
            ))
            .expect("record sibling cassette fixtures");
    }

    let fail_if_called = FakeProvider::builder("fake")
        .model("m")
        .error(sema_llm::types::LlmError::Api {
            status: 500,
            message: "provider must not be called on sibling replay".into(),
        })
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fail_if_called));
    let value = interp
        .eval_str_compiled(&format!(
            r#"(define a
                 (llm/with-cassette "{}" {{:mode :replay}}
                   (fn () (async/spawn
                            (fn () (llm/complete "prompt-a" {{:model "m"}}))))))
               (define b
                 (llm/with-cassette "{}" {{:mode :replay}}
                   (fn () (async/spawn
                            (fn () (llm/complete "prompt-b" {{:model "m"}}))))))
               (async/all (list a b))"#,
            path_a.display(),
            path_b.display()
        ))
        .expect("sibling replay tasks should retain distinct cassette scopes");
    let values = value.as_list().expect("two replay results");
    assert_eq!(values[0].as_str(), Some("alpha"));
    assert_eq!(values[1].as_str(), Some("beta"));

    let _ = std::fs::remove_file(path_a);
    let _ = std::fs::remove_file(path_b);
}

/// Two deferred recording scopes can target the same tape without last-writer-wins
/// data loss. Each scope loads before either task completes, so only append-only
/// persistence can retain both interactions.
#[test]
#[serial]
fn async_cassette_siblings_append_to_one_tape() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();
    let path = std::env::temp_dir().join(format!(
        "sema-cassette-shared-{}-{}.jsonl",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&path);

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(
        FakeProvider::builder("fake").model("m").echo().build(),
    ));
    interp
        .eval_str_compiled(&format!(
            r#"(define a
                 (llm/with-cassette "{}" {{:mode :record}}
                   (fn () (async/spawn
                            (fn () (llm/complete "prompt-a" {{:model "m"}}))))))
               (define b
                 (llm/with-cassette "{}" {{:mode :record}}
                   (fn () (async/spawn
                            (fn () (llm/complete "prompt-b" {{:model "m"}}))))))
               (async/all (list a b))"#,
            path.display(),
            path.display()
        ))
        .expect("both sibling recorders should finish");

    reset_runtime_state();
    register_test_provider(Box::new(
        FakeProvider::builder("fake")
            .model("m")
            .error(sema_llm::types::LlmError::Api {
                status: 500,
                message: "provider must not be called on shared-tape replay".into(),
            })
            .build(),
    ));
    let replayed = interp
        .eval_str_compiled(&format!(
            r#"(llm/with-cassette "{}" {{:mode :replay}}
                 (fn ()
                   (list (llm/complete "prompt-a" {{:model "m"}})
                         (llm/complete "prompt-b" {{:model "m"}}))))"#,
            path.display()
        ))
        .expect("one tape should retain both sibling recordings");
    let values = replayed.as_list().expect("two replayed values");
    assert_eq!(values[0].as_str(), Some("prompt-a"));
    assert_eq!(values[1].as_str(), Some("prompt-b"));

    let _ = std::fs::remove_file(path);
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

/// Every re-ask is a canonical cooperative completion. A timer that becomes
/// ready between attempt 0 and attempt 1 must run before the second provider
/// result, and both successful provider responses are accounted exactly once.
#[test]
#[serial]
fn extract_native_reask_parks_while_a_sibling_runs() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(100)
        .reply(r#"{"n":"invalid"}"#)
        .reply(r#"{"n":2}"#)
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (define before (:total-tokens (llm/session-usage)))
            (define out (channel/new 2))
            (async/spawn (fn () (sleep 150) (channel/send out "sibling")))
            (channel/send out
              (format "~a"
                (:n (llm/extract {:n {:type :number}} "root" {:retries 1}))))
            (list (channel/recv out) (channel/recv out)
                  (- (:total-tokens (llm/session-usage)) before))
            "#,
        )
        .expect("native extraction re-ask and sibling settle");

    let items = value.as_list().expect("ordered results and usage");
    assert_eq!(items[0].as_str(), Some("sibling"));
    assert_eq!(items[1].as_str(), Some("2"));
    assert_eq!(items[2].as_int(), Some(30));
    assert_eq!(recorder.call_count(), 2);
}

/// Re-asks rebuild the same fallback chain on every attempt; a failing Sema
/// provider is invoked before the native fallback for both rounds.
#[test]
#[serial]
fn extract_reask_reuses_mixed_provider_fallback() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply(r#"{"n":"invalid"}"#)
        .reply(r#"{"n":2}"#)
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (define sema-calls 0)
            (llm/define-provider :sema-provider
              {:complete
                (fn (_request)
                  (set! sema-calls (+ sema-calls 1))
                  (error "use fallback"))
               :default-model "sema-model"})
            (define result
              (llm/with-fallback [:sema-provider :fake]
                (fn ()
                  (llm/extract
                    {:n {:type :number}}
                    "root"
                    {:retries 1}))))
            (list (:n result) sema-calls)
            "#,
        )
        .expect("each extraction attempt traverses the mixed fallback chain");

    let items = value.as_list().expect("result and Sema call count");
    assert_eq!(items[0].as_int(), Some(2));
    assert_eq!(items[1].as_int(), Some(2));
    assert_eq!(recorder.call_count(), 2);
}

/// Disabling the verbose re-ask keeps the established terse system prompt and
/// otherwise preserves the original messages and JSON mode.
#[test]
#[serial]
fn extract_reask_false_preserves_exact_prompt_shape() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply(r#"{"n":"invalid"}"#)
        .reply(r#"{"n":2}"#)
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (:n
              (llm/extract
                {:n {:type :number}}
                "root"
                {:retries 1 :reask? #f}))
            "#,
        )
        .expect("terse extraction re-ask succeeds");
    assert_eq!(value.as_int(), Some(2));

    let requests = recorder.requests();
    assert_eq!(requests.len(), 2);
    let initial_system = requests[0]
        .system
        .as_deref()
        .expect("initial system prompt");
    assert_eq!(
        requests[1].system.as_deref(),
        Some(
            format!(
                "{initial_system}\n\nYour previous response had validation errors: \
                 key n: expected number, got string. Please fix."
            )
            .as_str()
        )
    );
    assert_eq!(requests[1].messages.len(), requests[0].messages.len());
    assert_eq!(requests[1].messages[0].role, requests[0].messages[0].role);
    assert_eq!(
        requests[1].messages[0].content.as_text(),
        requests[0].messages[0].content.as_text()
    );
    assert!(requests[1].json_mode);
}

/// Re-ask accounting stays attached to the budget captured by a task spawned
/// inside the scope, even when the task is awaited after that scope has ended.
#[test]
#[serial]
fn extract_reask_charges_spawn_captured_budget() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply_with_usage(r#"{"n":"invalid"}"#, 10, 5)
        .reply_with_usage(r#"{"n":2}"#, 10, 5)
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (define pending
              (llm/with-budget {:max-tokens 20}
                (fn ()
                  (async/spawn
                    (fn ()
                      (llm/extract
                        {:n {:type :number}}
                        "root"
                        {:retries 1}))))))
            (try (async/await pending) (catch error (:message error)))
            "#,
        )
        .expect("retry budget failure is catchable");

    assert!(value
        .as_str()
        .expect("budget error message")
        .contains("token budget exceeded: used 30 of 20 tokens"));
    assert_eq!(recorder.call_count(), 2);
}

/// Both request shapes produced by extraction are cached under the spawned
/// task's captured cache scope, so a later extraction can replay both attempts.
#[test]
#[serial]
fn extract_reask_uses_spawn_captured_cache_scope() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply(r#"{"n":"invalid"}"#)
        .reply(r#"{"n":2}"#)
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let primed = interp
        .eval_str_compiled(
            r#"
            (llm/cache-clear)
            (define pending
              (llm/with-cache {:ttl 3600}
                (fn ()
                  (async/spawn
                    (fn ()
                      (llm/extract
                        {:n {:type :number}}
                        "root"
                        {:retries 1}))))))
            (:n (async/await pending))
            "#,
        )
        .expect("spawned extraction primes both cache entries");
    assert_eq!(primed.as_int(), Some(2));
    assert_eq!(recorder.call_count(), 2);

    let fail = FakeProvider::builder("fake")
        .model("fake-chat")
        .error(sema_llm::types::LlmError::Api {
            status: 500,
            message: "provider must not be called on extraction cache replay".into(),
        })
        .build();
    let fail_recorder = fail.recorder();
    register_test_provider(Box::new(fail));
    let replayed = interp
        .eval_str_compiled(
            r#"
            (define pending
              (llm/with-cache {:ttl 3600}
                (fn ()
                  (async/spawn
                    (fn ()
                      (llm/extract
                        {:n {:type :number}}
                        "root"
                        {:retries 1}))))))
            (define result (async/await pending))
            (list (:n result)
                  (:hits (llm/cache-stats))
                  (:misses (llm/cache-stats)))
            "#,
        )
        .expect("spawned extraction replays both cache entries");
    let items = replayed.as_list().expect("result and cache stats");
    assert_eq!(items[0].as_int(), Some(2));
    assert_eq!(items[1].as_int(), Some(2));
    assert_eq!(items[2].as_int(), Some(2));
    assert_eq!(fail_recorder.call_count(), 0);
    interp
        .eval_str_compiled("(llm/cache-clear)")
        .expect("clean extraction cache fixture");
}

/// A spawned recording scope owns both extraction attempts, and replay can
/// consume both without reaching a provider after the dynamic extent unwinds.
#[test]
#[serial]
fn extract_reask_uses_spawn_captured_cassette_scope() {
    let path = std::env::temp_dir().join(format!(
        "sema-cassette-extract-reask-{}-{}.jsonl",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&path);

    let interp = Interpreter::new();
    reset_runtime_state();
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply(r#"{"n":"invalid"}"#)
        .reply(r#"{"n":2}"#)
        .build();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    let recorded = interp
        .eval_str_compiled(&format!(
            r#"
            (define pending
              (llm/with-cassette "{}" {{:mode :record}}
                (fn ()
                  (async/spawn
                    (fn ()
                      (llm/extract
                        {{:n {{:type :number}}}}
                        "root"
                        {{:retries 1}}))))))
            (:n (async/await pending))
            "#,
            path.display()
        ))
        .expect("spawned extraction records both attempts");
    assert_eq!(recorded.as_int(), Some(2));
    assert_eq!(recorder.call_count(), 2);

    reset_runtime_state();
    let fail = FakeProvider::builder("fake")
        .model("fake-chat")
        .error(sema_llm::types::LlmError::Api {
            status: 500,
            message: "provider must not be called on extraction cassette replay".into(),
        })
        .build();
    let fail_recorder = fail.recorder();
    register_test_provider(Box::new(fail));
    let replayed = interp
        .eval_str_compiled(&format!(
            r#"
            (define pending
              (llm/with-cassette "{}" {{:mode :replay}}
                (fn ()
                  (async/spawn
                    (fn ()
                      (llm/extract
                        {{:n {{:type :number}}}}
                        "root"
                        {{:retries 1}}))))))
            (:n (async/await pending))
            "#,
            path.display()
        ))
        .expect("spawned extraction replays both attempts");
    assert_eq!(replayed.as_int(), Some(2));
    assert_eq!(fail_recorder.call_count(), 0);

    let _ = std::fs::remove_file(path);
}

/// A Sema-defined provider can suspend and observe the caller's task context on
/// a later extraction attempt, not only on attempt 0.
#[test]
#[serial]
fn extract_sema_provider_reask_is_structural() {
    let interp = Interpreter::new();
    reset_runtime_state();

    let value = interp
        .eval_str_compiled(
            r#"
            (define calls 0)
            (llm/define-provider :sema-provider
              {:complete
                (fn (_request)
                  (set! calls (+ calls 1))
                  (if (= calls 1)
                      {:content "{\"n\":\"invalid\"}"
                       :usage {:prompt-tokens 3 :completion-tokens 2}}
                      (begin
                        (async/await
                          (async/spawn (fn () (sleep 10) nil)))
                        {:content
                          (string-append "{\"n\":" (context/get :answer) "}")
                         :usage {:prompt-tokens 3 :completion-tokens 2}})))
               :default-model "sema-model"})
            (define before (:total-tokens (llm/session-usage)))
            (define result
              (context/with {:answer "7"}
                (fn ()
                  (llm/extract {:n {:type :number}} "root" {:retries 1}))))
            (list (:n result) calls
                  (- (:total-tokens (llm/session-usage)) before))
            "#,
        )
        .expect("Sema extraction re-ask preserves runtime context");

    let items = value.as_list().expect("result, call count, and usage");
    assert_eq!(items[0].as_int(), Some(7));
    assert_eq!(items[1].as_int(), Some(2));
    assert_eq!(items[2].as_int(), Some(10));
}

/// Schema predicates are structural Sema calls: they may spawn/await and read
/// task-local context while preserving the one-provider-call success path.
#[test]
#[serial]
fn extract_custom_validator_can_suspend_and_observe_context() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply(r#"{"name":"Ada"}"#)
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (context/with {:prefix "ctx"}
              (fn ()
                (:name
                  (llm/extract
                    {:name
                      {:type :string
                       :validate
                         (fn (value)
                           (define pending
                             (async/spawn (fn () (sleep 10) (= value "Ada"))))
                           (and (= (context/get :prefix) "ctx")
                                (async/await pending)))}}
                    "root"
                    {:retries 0}))))
            "#,
        )
        .expect("suspending extraction validator succeeds");

    assert_eq!(value.as_str(), Some("Ada"));
    assert_eq!(recorder.call_count(), 1);
}

/// Cancellation during a schema predicate propagates out of extraction. It is
/// not converted into a validation failure and must not issue a re-ask.
#[test]
#[serial]
fn cancelling_extract_validator_does_not_reask_or_charge_again() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply(r#"{"name":"Ada"}"#)
        .reply(r#"{"name":"must-not-dispatch"}"#)
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let started = Instant::now();
    let value = interp
        .eval_str_compiled(
            r#"
            (define pending
              (async/spawn
                (fn ()
                  (llm/extract
                    {:name
                      {:type :string
                       :validate (fn (_value) (sleep 1000) #t)}}
                    "root"
                    {:retries 1}))))
            (async/spawn (fn () (sleep 20) (async/cancel pending)))
            (list (try (async/await pending) (catch error :cancelled))
                  (:total-tokens (llm/session-usage)))
            "#,
        )
        .expect("cancelled extraction validator settles");

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "cancellation must not wait out the validator sleep"
    );
    let items = value.as_list().expect("cancel result and usage");
    assert_eq!(items[0], sema_core::Value::keyword("cancelled"));
    assert_eq!(items[1].as_int(), Some(15));
    assert_eq!(recorder.call_count(), 1, "cancellation must not re-ask");
}

/// JSON decoding failures precede schema validation and remain terminal even
/// when validation retries are enabled.
#[test]
#[serial]
fn extract_invalid_json_does_not_reask() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply("not json")
        .reply(r#"{"n":2}"#)
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (try
              (llm/extract {:n {:type :number}} "root" {:retries 2})
              (catch error (:message error)))
            "#,
        )
        .expect("invalid JSON is catchable");

    assert!(value
        .as_str()
        .expect("error message")
        .contains("failed to parse LLM JSON response"));
    assert_eq!(recorder.call_count(), 1, "parse errors must not re-ask");
}

/// Validator failures accumulate in schema order, and a failing predicate does
/// not prevent later predicates from running.
#[test]
#[serial]
fn extract_validator_errors_are_ordered_and_non_short_circuiting() {
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .reply(r#"{"a":"x","b":"y"}"#)
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let value = interp
        .eval_str_compiled(
            r#"
            (define second-ran #f)
            (define message
              (try
                (llm/extract
                  {:a {:type :string
                       :validate (fn (_value) (error "first boom"))}
                   :b {:type :string
                       :validate (fn (_value) (set! second-ran #t) #f)
                       :message "second false"}}
                  "root"
                  {:retries 0})
                (catch error (:message error))))
            (list message second-ran)
            "#,
        )
        .expect("validator failures are catchable");

    let items = value.as_list().expect("message and continuation marker");
    let message = items[0].as_str().expect("validation error message");
    let first = message
        .find("key a: validation error:")
        .expect("first validator error is present");
    let second = message
        .find("key b: second false")
        .expect("second validator error is present");
    assert!(
        first < second,
        "validator errors must preserve schema order"
    );
    assert_eq!(items[1].as_bool(), Some(true));
    assert_eq!(recorder.call_count(), 1);
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

/// Budget enforcement crosses the spawn + AwaitIo-yield boundary: a completion
/// running inside an `async/spawn`'d task, under `llm/with-budget`, must charge
/// the CAPTURED budget frame (dispatch-time `Rc` snapshot, settled in the
/// poller) — not whatever thread-local scope is live when the future lands.
/// Composes ASYNC-1's per-task LLM scope capture with the offload path; closes
/// the "budget-across-yield" gap the ADR #68/#69 plans tracked as step 7.
#[test]
#[serial]
fn budget_enforced_across_spawn_and_yield() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    // 100 prompt + 50 completion tokens >> the 5-token budget.
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .chat_delay(50)
        .reply_with_usage("over", 100, 50)
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (llm/cache-clear)
        (try
          (begin
            (llm/with-budget {:max-tokens 5}
              (fn () (async/await (async/spawn (fn () (llm/complete "big"))))))
            "no-error")
          (catch e "budget-raised"))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("budget program evaluated");
    assert_eq!(
        val.as_str(),
        Some("budget-raised"),
        "a spawned completion must charge the captured budget frame and raise on overrun"
    );
}

/// Embed parity for the ASYNC-1 captured-frame accounting: an `llm/embed` inside
/// an `async/spawn`'d task under `llm/with-budget` must charge the DISPATCH-TIME
/// budget frame in the poller — the same guarantee `llm/complete` has
/// (`budget_enforced_across_spawn_and_yield`). Previously the embed poller
/// charged whatever budget scope was live when the future landed.
#[test]
#[serial]
fn embed_budget_enforced_across_spawn_and_yield() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-embed")
        .embed_delay(50)
        .embed_with_tokens(vec![vec![0.1, 0.2, 0.3]], 100)
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (llm/cache-clear)
        (try
          (begin
            (llm/with-budget {:max-tokens 5}
              (fn () (async/await (async/spawn (fn () (llm/embed "big"))))))
            "no-error")
          (catch e "budget-raised"))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("embed budget program evaluated");
    assert_eq!(
        val.as_str(),
        Some("budget-raised"),
        "a spawned embed must charge the captured budget frame and raise on overrun"
    );
}

/// `llm/with-rate-limit`'s pacing gap must not stall a sibling task: the SECOND of
/// two rate-limited `llm/complete` calls has to wait out the configured interval
/// before its send, and a concurrently-spawned non-LLM sibling (a short `sleep`)
/// must complete WHILE that wait is elapsing — proving the wait is spent off the
/// VM thread (inside the offloaded future), not as a blocking `thread::sleep` on
/// it. Ordering is asserted via a channel (deterministic), never a wall-clock
/// duration: the sibling's channel send must land strictly BEFORE the second
/// completion's. Both completions still complete correctly (recorder sees 2
/// requests) — the yield behavior is under test, not precise pacing.
#[test]
#[serial]
fn rate_limit_pacing_gap_lets_sibling_complete() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    // Echo mode: reply text == prompt text, so completions are identifiable by
    // content regardless of channel receive order.
    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .echo()
        .build();
    let recorder = fake.recorder();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // 5 rps => 200ms minimum spacing. Spawned in this order, task "a" reserves
    // its slot first (no wait) and task "b" reserves second (~200ms wait, spent
    // inside b's offloaded future). The sibling's 20ms sleep is short enough to
    // resolve well inside that 200ms gap if — and only if — the VM thread kept
    // running siblings while b's pacing wait elapsed.
    let program = r#"
        (let ((out (channel/new 8)))
          (llm/with-rate-limit 5.0
            (fn ()
              (async/all
                (list
                  (async/spawn (fn () (channel/send out (llm/complete "a"))))
                  (async/spawn (fn () (channel/send out (llm/complete "b"))))
                  (async/spawn (fn () (sleep 20) (channel/send out "sibling")))))))
          (list (channel/recv out) (channel/recv out) (channel/recv out)))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("rate-limited concurrent completes evaluated");
    let received: Vec<String> = result
        .as_list()
        .expect("three channel receives")
        .iter()
        .map(|v| v.as_str().expect("string value").to_string())
        .collect();
    assert_eq!(received.len(), 3);

    let sibling_pos = received
        .iter()
        .position(|v| v == "sibling")
        .expect("sibling value received");
    let second_pos = received
        .iter()
        .position(|v| v == "b")
        .expect("second completion (b) received");
    assert!(
        sibling_pos < second_pos,
        "the non-LLM sibling must complete before the rate-limited second call's \
         pacing gap elapses (got order {received:?})"
    );
    // Both completions succeeded (the fake never errors) and the provider was
    // actually dispatched twice — pacing didn't drop or dedupe either call.
    assert!(received.contains(&"a".to_string()));
    assert_eq!(
        recorder.call_count(),
        2,
        "both rate-limited completions must reach the provider exactly once each"
    );
}

/// Sync-context regression: `llm/with-rate-limit` at TOP LEVEL (no scheduler task)
/// still works exactly as before — both completions succeed and reach the
/// provider. `enforce_rate_limit` (the blocking sync gate) is untouched by the
/// async-path fix (`reserve_rate_limit_wait_ms`, used only by
/// `do_complete_async_yield`/`stream_run_begin`); this is the functional
/// companion to the unit test `enforce_rate_limit_survives_backward_clock`, which
/// covers the sync gate's own edge-case robustness. No duration is asserted
/// (matching the removed flaky test this gap was tracked under) — only that
/// pacing two calls at a real interval still returns correct results.
#[test]
#[serial]
fn sync_rate_limit_still_works() {
    let _cap = sema_otel::testing::install();
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-chat")
        .echo()
        .build();
    let recorder = fake.recorder();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // A high rps keeps the real sleep this exercises (interval ~1ms) negligible.
    let program = r#"
        (llm/with-rate-limit 1000.0
          (fn () (list (llm/complete "a") (llm/complete "b"))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("sync rate-limited completes evaluated");
    let res = result.as_list().expect("results list");
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].as_str(), Some("a"));
    assert_eq!(res[1].as_str(), Some("b"));
    assert_eq!(recorder.call_count(), 2);
}
