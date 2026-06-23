//! Gate for async span NESTING across `async/spawn` boundaries.
//!
//! A span opened inside a spawned task must nest under the spawner's active span —
//! same trace, `parent_span_id` = the spawner span's id — so `(with-span … (async/map
//! …))` produces ONE connected trace tree, not N disconnected single-span traces.
//! (Spawned tasks are seeded with the spawner's current span context as parent;
//! see `sema-otel` `current_conversation_scope`.) The companion invariant: sibling
//! tasks spawned at the TOP level — with no active span — stay in DISTINCT traces
//! (per-task isolation preserved).

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use serial_test::serial;

fn zero_id(s: &str) -> bool {
    s.chars().all(|c| c == '0')
}

/// `(with-span "parent" … (async/spawn (… (with-span "child" …))))` → the child span
/// shares the parent's trace and is parented to it.
#[test]
#[serial]
fn spawned_span_nests_under_spawner() {
    let cap = sema_otel::testing::install();
    let interp = Interpreter::new();
    interp
        .eval_str_compiled(
            r#"(with-span "parent" {}
                 (async/all (list (async/spawn (fn () (with-span "child" {} 1))))))"#,
        )
        .expect("nested-span program evaluated");

    let spans = cap.spans_json();
    let parent = spans
        .iter()
        .find(|s| s["name"] == "parent")
        .expect("parent span present");
    let child = spans
        .iter()
        .find(|s| s["name"] == "child")
        .expect("child span present");

    // Same trace.
    assert_eq!(
        parent["trace_id"], child["trace_id"],
        "child must share the parent's trace_id (one connected trace)"
    );
    // Child parented to the parent span.
    assert_eq!(
        child["parent_span_id"].as_str(),
        parent["span_id"].as_str(),
        "child.parent_span_id must equal parent.span_id"
    );
    // Parent itself is a root.
    assert!(
        zero_id(parent["parent_span_id"].as_str().unwrap_or("")),
        "the parent span should be a trace root here"
    );
}

/// Two tasks spawned at the TOP level (no enclosing span) stay in DISTINCT traces —
/// the per-task isolation that must survive the nesting change.
#[test]
#[serial]
fn top_level_sibling_spawns_are_isolated_traces() {
    let cap = sema_otel::testing::install();
    let interp = Interpreter::new();
    interp
        .eval_str_compiled(
            r#"(async/all
                 (list (async/spawn (fn () (with-span "a" {} 1)))
                       (async/spawn (fn () (with-span "b" {} 2)))))"#,
        )
        .expect("sibling-span program evaluated");

    let spans = cap.spans_json();
    let a = spans.iter().find(|s| s["name"] == "a").expect("span a");
    let b = spans.iter().find(|s| s["name"] == "b").expect("span b");
    assert_ne!(
        a["trace_id"], b["trace_id"],
        "top-level sibling spawns must be in distinct traces (isolation preserved)"
    );
    assert!(zero_id(a["parent_span_id"].as_str().unwrap_or("")));
    assert!(zero_id(b["parent_span_id"].as_str().unwrap_or("")));
}
