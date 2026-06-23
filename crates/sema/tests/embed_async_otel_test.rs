//! Gate for concurrent single-shot `llm/embed` with full per-task OTel tracing.
//!
//! Two concurrent `llm/embed`s (spawned tasks) must (1) OVERLAP on the
//! cooperative scheduler — wall ≈ max(delay), not the sum — and (2) each emit a
//! DISTINCT, correctly-isolated `embeddings` span: distinct trace_id + span_id,
//! neither parenting the other, each carrying its OWN input-token total (the
//! per-task otel-isolation proof). The sync path is unchanged: one embed outside
//! an async context emits exactly one correct span.
//!
//! Deterministic + keyless (a delayed FakeProvider embed). Own binary — the
//! in-memory exporter and `sema_otel::testing::install()` are process-global, so
//! the timing/overlap test must not share a process with unrelated span capture.

#![cfg(not(target_arch = "wasm32"))]

use std::time::Instant;

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;
use serial_test::serial;

/// Two concurrent `llm/embed`s overlap AND produce two distinct, isolated
/// `embeddings` spans. The FakeProvider injects a 300 ms delay into each
/// `embed()`, so two serial embeds would take ~600 ms; overlapping ~300 ms.
/// Each embed is scripted with a DISTINCT prompt-token count (7 / 11) so the two
/// spans must carry their own input-token totals — proving the per-task otel TLS
/// swap kept them isolated rather than cross-contaminating one shared stack.
#[test]
#[serial]
fn two_concurrent_embeds_overlap_with_isolated_spans() {
    let cap = sema_otel::testing::install();

    // Two scripted embeds, in spawn order: distinct vectors + distinct token
    // counts. `async/all` preserves spawn (input) order, so embed #0 (7 tokens)
    // resolves first in the result, embed #1 (11 tokens) second.
    let fake = FakeProvider::builder("fake")
        .model("fake-embed")
        .embed_delay(300)
        .embed_with_tokens(vec![vec![0.1, 0.2, 0.3]], 7)
        .embed_with_tokens(vec![vec![0.4, 0.5, 0.6]], 11)
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (let ((t0 (sys/elapsed)))
          (let ((res (async/all
                       (map (fn (t) (async/spawn (fn () (embedding/length (llm/embed t)))))
                            (list "alpha" "beta")))))
            (list res (floor (/ (- (sys/elapsed) t0) 1000000)))))
    "#;

    let t0 = Instant::now();
    let result = interp
        .eval_str_compiled(program)
        .expect("concurrent embed program evaluated");
    let wall_ms = t0.elapsed().as_millis();

    // (1a) Correctness: two embeddings, each 3-dim (24 bytes / 8).
    let outer = result.as_list().expect("result is (results wall-ms)");
    let res = outer[0].as_list().expect("results list");
    assert_eq!(res.len(), 2, "expected two embedding results");
    assert_eq!(res[0].as_int(), Some(3), "embed #0 has 3 dims");
    assert_eq!(res[1].as_int(), Some(3), "embed #1 has 3 dims");

    // (1b) Overlap: serial floor ~600 ms; overlapping ~300 ms. Generous ceiling.
    assert!(
        wall_ms < 500,
        "expected overlapped wall-clock < 500 ms (serial floor ~600 ms), got {wall_ms} ms"
    );

    // (2) Two distinct, isolated `embeddings` spans.
    let spans = cap.spans_json();
    let embed_spans: Vec<&serde_json::Value> = spans
        .iter()
        .filter(|s| s["attributes"]["gen_ai.operation.name"] == "embeddings")
        .collect();
    assert_eq!(
        embed_spans.len(),
        2,
        "expected exactly two embeddings spans, got {}",
        embed_spans.len()
    );

    let trace_ids: Vec<&serde_json::Value> = embed_spans.iter().map(|s| &s["trace_id"]).collect();
    let span_ids: Vec<&serde_json::Value> = embed_spans.iter().map(|s| &s["span_id"]).collect();

    // Distinct span ids (always) and distinct trace ids (each detached embed is
    // its own root → its own trace).
    assert_ne!(
        span_ids[0], span_ids[1],
        "the two spans must have distinct span_ids"
    );
    assert_ne!(
        trace_ids[0], trace_ids[1],
        "the two embeds run in distinct tasks → distinct traces"
    );

    // Neither span parents the other (no cross-task nesting from a shared stack).
    for s in &embed_spans {
        let parent = &s["parent_span_id"];
        assert_ne!(parent, span_ids[0], "span parented under the other embed");
        assert_ne!(parent, span_ids[1], "span parented under the other embed");
    }

    // Each span carries its OWN input-token total (7 and 11), proving the
    // per-task otel context swap kept the two spans isolated.
    let mut tokens: Vec<i64> = embed_spans
        .iter()
        .map(|s| {
            s["attributes"]["gen_ai.usage.input_tokens"]
                .as_i64()
                .expect("input_tokens present")
        })
        .collect();
    tokens.sort_unstable();
    assert_eq!(
        tokens,
        vec![7, 11],
        "each embed span must carry its own input-token count (7 and 11), got {tokens:?}"
    );
    // And output tokens are zero on both (embeddings report input only).
    for s in &embed_spans {
        assert_eq!(s["attributes"]["gen_ai.usage.output_tokens"], 0);
        assert_eq!(s["kind"], "client");
        assert_eq!(s["attributes"]["gen_ai.provider.name"], "fake");
    }
}

/// `llm/embed` must be a FIRST-CLASS native function, not a macro: `(procedure?
/// llm/embed)` is `#t`, it is `map`-pable at top level (sync), and — the headline
/// RAG pattern — usable as the function argument to `async/pool-map`, where it runs
/// concurrently under bounded fan-out and returns results in input order.
#[test]
#[serial]
fn embed_is_first_class_and_pool_mappable() {
    let _cap = sema_otel::testing::install();

    let fake = FakeProvider::builder("fake")
        .model("fake-embed")
        // Six scripted embeds: two for the sync `(map llm/embed …)` call, four for
        // the `async/pool-map` over four chunks. Each is a distinct 3-dim vector.
        .embed_delay(300)
        .embed_with_tokens(vec![vec![0.1, 0.2, 0.3]], 3)
        .embed_with_tokens(vec![vec![0.4, 0.5, 0.6]], 3)
        .embed_with_tokens(vec![vec![0.7, 0.8, 0.9]], 3)
        .embed_with_tokens(vec![vec![1.0, 1.1, 1.2]], 3)
        .embed_with_tokens(vec![vec![1.3, 1.4, 1.5]], 3)
        .embed_with_tokens(vec![vec![1.6, 1.7, 1.8]], 3)
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // (a) `llm/embed` is a procedure (was #f when it was a router macro).
    let is_proc = interp
        .eval_str_compiled("(procedure? llm/embed)")
        .expect("procedure? llm/embed evaluated");
    assert_eq!(
        is_proc.as_bool(),
        Some(true),
        "llm/embed must be a first-class procedure"
    );

    // (b) `(map llm/embed ...)` works at top level (sync path), in order.
    let mapped = interp
        .eval_str_compiled(r#"(map embedding/length (map llm/embed (list "a" "b")))"#)
        .expect("map llm/embed evaluated");
    let dims = mapped.as_list().expect("list of lengths");
    assert_eq!(dims.len(), 2);
    assert_eq!(dims[0].as_int(), Some(3));
    assert_eq!(dims[1].as_int(), Some(3));

    // (c) `(async/pool-map llm/embed chunks N)` — the headline RAG pattern — runs
    // concurrently (overlap wall ≈ ceil(4/2)*300 = 600 ms, not 4*300 = 1200 ms
    // serial) and returns results in INPUT order.
    let program = r#"
        (let ((t0 (sys/elapsed)))
          (let ((res (async/pool-map llm/embed (list "w" "x" "y" "z") 2)))
            (list (map embedding/length res)
                  (floor (/ (- (sys/elapsed) t0) 1000000)))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("async/pool-map llm/embed evaluated");
    let outer = result.as_list().expect("(dims wall-ms)");
    let dims = outer[0].as_list().expect("dims list");
    assert_eq!(dims.len(), 4, "four embeddings in input order");
    for d in dims {
        assert_eq!(d.as_int(), Some(3));
    }
    let wall_ms = outer[1].as_int().expect("wall ms");
    assert!(
        wall_ms < 1000,
        "expected overlapped wall < 1000 ms (serial floor ~1200 ms), got {wall_ms} ms"
    );
}

/// Sync path unchanged: a single `(llm/embed "x")` OUTSIDE any async context emits
/// exactly one correct `embeddings` span via the synchronous path.
#[test]
#[serial]
fn sync_embed_outside_async_emits_one_span() {
    let cap = sema_otel::testing::install();

    let fake = FakeProvider::builder("fake")
        .model("fake-embed")
        .embed_with_tokens(vec![vec![0.1, 0.2, 0.3]], 5)
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    interp
        .eval_str_compiled(r#"(llm/embed "hello world")"#)
        .expect("sync embed should run against the fake");

    let embed_spans: Vec<serde_json::Value> = cap
        .spans_json()
        .into_iter()
        .filter(|s| s["attributes"]["gen_ai.operation.name"] == "embeddings")
        .collect();
    assert_eq!(
        embed_spans.len(),
        1,
        "exactly one embeddings span on the sync path"
    );
    let embed = &embed_spans[0];
    assert_eq!(embed["kind"], "client");
    assert_eq!(embed["attributes"]["gen_ai.provider.name"], "fake");
    assert_eq!(embed["attributes"]["gen_ai.response.model"], "fake-embed");
    assert_eq!(embed["attributes"]["gen_ai.usage.input_tokens"], 5);
    assert_eq!(embed["attributes"]["gen_ai.usage.output_tokens"], 0);
}

/// Live overlap (run with `--ignored` and OPENAI_API_KEY set): two concurrent
/// real `text-embedding-3-small` embeds overlap and emit two distinct spans.
#[test]
#[ignore]
#[serial]
fn live_two_concurrent_real_embeds_overlap() {
    if std::env::var("OPENAI_API_KEY").is_err() {
        eprintln!("skipping: OPENAI_API_KEY not set");
        return;
    }
    let cap = sema_otel::testing::install();
    let interp = Interpreter::new();

    let program = r#"
        (llm/configure-embeddings :openai {:api-key (env "OPENAI_API_KEY")
                                           :model "text-embedding-3-small"})
        (let ((t0 (sys/elapsed)))
          (let ((res (async/all
                       (map (fn (t) (async/spawn (fn () (embedding/length (llm/embed t)))))
                            (list "the quick brown fox" "lorem ipsum dolor")))))
            (list res (floor (/ (- (sys/elapsed) t0) 1000000)))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("live concurrent embeds evaluated");
    let outer = result.as_list().expect("(results wall-ms)");
    let res = outer[0].as_list().expect("results");
    assert_eq!(res.len(), 2, "two embeddings");

    let embed_spans: Vec<serde_json::Value> = cap
        .spans_json()
        .into_iter()
        .filter(|s| s["attributes"]["gen_ai.operation.name"] == "embeddings")
        .collect();
    assert_eq!(embed_spans.len(), 2, "two embeddings spans");
    assert_ne!(
        embed_spans[0]["span_id"], embed_spans[1]["span_id"],
        "distinct span_ids"
    );
    assert_ne!(
        embed_spans[0]["trace_id"], embed_spans[1]["trace_id"],
        "distinct trace_ids"
    );
}
