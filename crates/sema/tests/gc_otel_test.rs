//! GC observability: every cycle-collector pass that actually runs emits a
//! `gc.collect` span (retroactively timed, trigger + stats as attributes), so
//! collector behavior lands on the same timeline as LLM/tool spans.
//! Deterministic (in-memory exporter, no network, no timing asserts). Own
//! binary (the global provider is process-global).

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;

/// Every trigger name `GcTrigger::as_str` can produce.
const VALID_TRIGGERS: [&str; 8] = [
    "threshold",
    "eval-return",
    "interpreter-drop",
    "notebook-cell",
    "notebook-reset",
    "agent-turn",
    "scheduler-idle",
    "explicit",
];

#[test]
fn gc_pass_emits_gc_collect_span_with_trigger_and_stats() {
    let cap = sema_otel::testing::install();
    let interp = Interpreter::new();

    // Recursive-closure churn: each `churn` call creates a self-recursive
    // local closure that becomes a garbage Rc cycle immediately — the exact
    // shape the collector reclaims. The self-call is non-tail (a tail-only
    // self-recursion elides its self capture, issue #62, and forms no cycle).
    // 100 iterations stay under the collection threshold, so the cycles are
    // still uncollected when the explicit `(gc/collect)` runs.
    let src = r#"
        (define (churn)
          (define (loop n) (if (<= n 0) 0 (+ 1 (loop (- n 1)))))
          (loop 3))
        (define (run n) (if (<= n 0) 0 (begin (churn) (run (- n 1)))))
        (run 100)
        (gc/collect)
    "#;
    interp
        .eval_str_compiled(src)
        .expect("eval churn + gc/collect");

    let gc_spans: Vec<_> = cap
        .spans_json()
        .into_iter()
        .filter(|s| s["name"] == "gc.collect")
        .collect();
    assert!(
        !gc_spans.is_empty(),
        "at least one collector pass ran (the explicit (gc/collect) always does)"
    );

    // Every pass span carries a valid trigger and internally consistent stats.
    for span in &gc_spans {
        let attrs = &span["attributes"];
        let trigger = attrs["gc.trigger"]
            .as_str()
            .expect("gc.trigger is a string");
        assert!(
            VALID_TRIGGERS.contains(&trigger),
            "unknown gc.trigger {trigger:?}"
        );
        let int = |key: &str| {
            attrs[key]
                .as_i64()
                .unwrap_or_else(|| panic!("{key} is an integer on {attrs}"))
        };
        assert!(
            int("gc.registry_before") >= int("gc.candidates"),
            "live candidates never exceed the registry they were snapshotted from"
        );
        assert!(int("gc.traced") >= int("gc.collected"));
        assert!(attrs["gc.aborted"].is_boolean(), "gc.aborted is a bool");
    }

    // The explicit (gc/collect) pass is attributed to its safe point...
    let explicit = gc_spans
        .iter()
        .find(|s| s["attributes"]["gc.trigger"] == "explicit")
        .expect("the (gc/collect) pass emits an explicit-trigger span");
    assert_eq!(explicit["attributes"]["gc.aborted"], false);

    // ...and the churned cycles show up as reclaimed garbage on some pass
    // (the explicit one, unless a threshold pass got there first).
    assert!(
        gc_spans
            .iter()
            .any(|s| s["attributes"]["gc.collected"].as_i64().unwrap_or(0) > 0),
        "the churned recursive-closure cycles were reclaimed on an observed pass"
    );
}
