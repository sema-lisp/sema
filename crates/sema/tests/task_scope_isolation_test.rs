//! Gate for per-task OTel span-stack ISOLATION under cooperative interleaving.
//!
//! The unified runtime swaps each task's OTel context (span stack + ids) into and out
//! of the one thread-local per quantum. Two tasks that each open a span and then yield
//! mid-span (here via `async/sleep`) are forced to interleave: task A opens its span,
//! parks; task B opens its span, parks; then each resumes and opens a CHILD span while
//! its own outer span is still active. Without the per-task swap both tasks push onto
//! one shared thread-local stack, so a resuming task's child span mis-parents to the
//! SIBLING's span (cross-trace corruption). With the swap each task sees only its own
//! stack, so every child parents to ITS OWN outer span in ITS OWN trace.
//!
//! Companion to `async_span_nesting_test.rs`, which covers the spawn-time PARENT SEED
//! (no interleave); this covers the per-quantum SWAP that the seed alone cannot.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use serial_test::serial;

/// Two top-level spawned tasks, each opening an outer span, sleeping mid-span (forcing
/// an interleave), then opening a child span. Each child must parent to its OWN outer
/// span and share its OWN trace — never the sibling's.
#[test]
#[serial]
fn interleaved_spawns_keep_span_stacks_isolated() {
    let cap = sema_otel::testing::install();
    let interp = Interpreter::new();
    interp
        .eval_str_compiled(
            r#"(async/all
                 (list
                   (async/spawn
                     (fn () (with-span "a" {}
                              (async/sleep 5)
                              (with-span "a-child" {} 1))))
                   (async/spawn
                     (fn () (with-span "b" {}
                              (async/sleep 5)
                              (with-span "b-child" {} 2))))))"#,
        )
        .expect("interleaved-span program evaluated");

    let spans = cap.spans_json();
    let find = |name: &str| {
        spans
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("span {name} present"))
    };
    let a = find("a");
    let b = find("b");
    let a_child = find("a-child");
    let b_child = find("b-child");

    // Each outer span is a distinct top-level trace root.
    assert_ne!(
        a["trace_id"], b["trace_id"],
        "the two top-level spawns must be distinct traces"
    );

    // a-child parents to a (same trace) — NOT to b, even though b's span was opened on
    // the shared thread-local stack in between a's open and a's resume.
    assert_eq!(
        a_child["parent_span_id"].as_str(),
        a["span_id"].as_str(),
        "a-child must parent to a's span, not the interleaved sibling's"
    );
    assert_eq!(
        a_child["trace_id"], a["trace_id"],
        "a-child must stay in a's trace"
    );

    // b-child parents to b (same trace) — symmetric.
    assert_eq!(
        b_child["parent_span_id"].as_str(),
        b["span_id"].as_str(),
        "b-child must parent to b's span, not the interleaved sibling's"
    );
    assert_eq!(
        b_child["trace_id"], b["trace_id"],
        "b-child must stay in b's trace"
    );

    // And the cross-trace mis-parent the bug produced must be absent.
    assert_ne!(
        a_child["trace_id"], b["trace_id"],
        "a-child must not leak into b's trace"
    );
    assert_ne!(
        b_child["trace_id"], a["trace_id"],
        "b-child must not leak into a's trace"
    );
}
