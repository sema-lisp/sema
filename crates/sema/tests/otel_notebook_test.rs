//! M4 acceptance: a notebook "Run All" produces one root trace whose root has one
//! child span per executed cell. Own binary so the global provider is isolated.

#![cfg(not(target_arch = "wasm32"))]

use sema_notebook::{Engine, Notebook};

#[test]
fn notebook_run_all_emits_one_root_with_cell_children() {
    let cap = sema_otel::testing::install();

    let mut nb = Notebook::new("otel-test");
    nb.add_code_cell("(+ 1 2)");
    nb.add_code_cell("(* 2 3)");
    nb.add_code_cell("(- 10 4)");
    let mut engine = Engine::new(nb);

    let results = engine.eval_all();
    assert_eq!(results.len(), 3, "all three code cells should evaluate");

    let spans = cap.spans_json();
    let root = spans
        .iter()
        .find(|s| s["name"] == "notebook.run_all")
        .expect("a notebook.run_all root span");
    assert_eq!(root["kind"], "internal");
    let root_id = root["span_id"].as_str().unwrap();

    // Exactly one cell span per executed cell, each a child of the run_all root.
    let cell_children: Vec<_> = spans
        .iter()
        .filter(|s| {
            s["name"]
                .as_str()
                .is_some_and(|n| n.starts_with("notebook.cell "))
                && s["parent_span_id"] == root_id
        })
        .collect();
    assert_eq!(
        cell_children.len(),
        3,
        "expected one cell span per cell under the root, got {}: {:#?}",
        cell_children.len(),
        spans
    );
    for c in &cell_children {
        assert_eq!(c["kind"], "internal");
        assert_eq!(c["trace_id"], root["trace_id"]);
    }
}
