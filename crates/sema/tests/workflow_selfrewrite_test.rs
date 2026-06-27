//! Self-rewrite primitives acceptance tests.
//!
//! Tests the three new capabilities introduced for dynamic self-rewriting workflows:
//!   1. `(workflow/check src)` — returns diagnostics as DATA (list of maps).
//!   2. `{:schema :sema-form}` in `step` — parses LLM output as Sema forms.
//!   3. `(workflow/run-form form)` — evaluates a form value at runtime.
//!
//! The first two tests are unit-style (Interpreter directly, no FakeProvider). The
//! third is a full integration test through the FakeProvider workflow harness.

mod workflow_common;
use workflow_common as wc;

use sema_eval::Interpreter;
use sema_llm::builtins::reset_runtime_state;
use sema_llm::fake::FakeProvider;

// A minimal workflow that is syntactically valid and passes workflow/check.
const CLEAN_WF_SRC: &str =
    r#"(defworkflow gen "g" {:phases ["P"]} (phase "P") {:status :success})"#;

// ── test 1: workflow/check returns empty list for clean source ────────────────

#[test]
fn workflow_check_returns_empty_for_clean_source() {
    let interp = Interpreter::new();
    reset_runtime_state();
    // Escape quotes for the Sema string literal.
    let escaped = CLEAN_WF_SRC.replace('"', "\\\"");
    let result = interp
        .eval_str_compiled(&format!(r#"(workflow/check "{escaped}")"#))
        .expect("eval should not fail");
    let len = result.as_seq().map(|s| s.len()).unwrap_or(99);
    assert_eq!(
        len, 0,
        "expected empty diag list for clean source, got: {result}"
    );
}

// ── test 2: workflow/check returns E-PHASE-ARITY diagnostic as DATA ──────────

#[test]
fn workflow_check_returns_e_phase_arity_as_data() {
    let interp = Interpreter::new();
    reset_runtime_state();
    // The #1 phase-arity trap: (phase "P" 1) — phase takes exactly 1 arg.
    let result = interp
        .eval_str_compiled(
            r#"(workflow/check "(defworkflow d \"x\" {} (phase \"P\" 1) {:status :ok})")"#,
        )
        .expect("eval should not fail");
    let seq = result.as_seq().expect("should be a list of diagnostics");
    assert!(!seq.is_empty(), "expected at least one diagnostic");
    // The first map must carry :code "E-PHASE-ARITY".
    let first = &seq[0];
    let code = first
        .as_map_rc()
        .and_then(|m| {
            m.get(&sema_core::Value::keyword("code"))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        })
        .unwrap_or_default();
    assert_eq!(
        code, "E-PHASE-ARITY",
        "expected E-PHASE-ARITY diagnostic code, got: {code:?}. Full result: {result}"
    );
    // Severity must be :error (not :warning).
    let severity = first
        .as_map_rc()
        .and_then(|m| {
            m.get(&sema_core::Value::keyword("severity"))
                .and_then(|v| v.as_keyword().map(|s| s.to_string()))
        })
        .unwrap_or_default();
    assert_eq!(severity, "error", "E-PHASE-ARITY should be severity :error");
}

// ── test 3: :sema-form step + workflow/check on data + workflow/run-form ──────

#[test]
fn sema_form_step_check_and_run_form_succeed() {
    // The FakeProvider replies with a valid workflow source string. The :sema-form
    // schema causes the step to parse it via read-many, producing a list of forms.
    // workflow/check on the list returns empty (clean). workflow/run-form evaluates
    // the forms, executing the defworkflow (which immediately runs and returns
    // {:status :success}).
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        // The model's reply: a valid workflow source string (no markdown fences).
        .reply(CLEAN_WF_SRC)
        .build();

    let src = r#"
        (defworkflow self-rewriter
          "emit and run a sub-workflow"
          {:phases ["Emit" "Run"]}
          (phase "Emit")
          (def f (step "emit a workflow" {:schema :sema-form :name "emitter"}))
          (phase "Run")
          (def diags (workflow/check f))
          (def run-result (workflow/run-form f))
          {:status :success
           :check-clean (null? diags)
           :ran (:status run-result)})
    "#;

    let out = wc::run_once(src, fake, "wf_selfrewrite");

    // The outer workflow must succeed.
    let run_ended = wc::events_of(&out.events, "run.ended");
    assert!(
        !run_ended.is_empty(),
        "expected at least one run.ended event"
    );
    assert_eq!(
        run_ended.last().unwrap()["status"],
        "success",
        "outer workflow should end with status success"
    );

    // result.json from the outer run should indicate success.
    assert_eq!(
        out.result["status"], "success",
        "result.json status should be success; got: {}",
        out.result
    );

    // The emitter step must have fired (one agent.result event).
    let agent_results = wc::events_of(&out.events, "agent.result");
    assert!(
        !agent_results.is_empty(),
        "expected at least one agent.result (the emitter step)"
    );
}
