//! `sema workflow check` — a STATIC validator for dynamic-workflow `.sema` files.
//!
//! Runs entirely on the parsed `Value` AST (via `sema_reader::read_many_with_spans_recover`)
//! — it never evaluates the workflow, configures a provider, or emits a journal event, so
//! it is instant, side-effect-free, and safe to run on untrusted source. It exists to give
//! a workflow author (often a coding agent) a fast feedback loop that catches the traps the
//! runtime only surfaces at eval time — chiefly the `(phase "x" body…)` arity trap, since
//! `phase` is a one-argument marker.
//!
//! Design (kept deliberately simple): one recursive visitor carries an `in_workflow` flag.
//! Marker checks (`phase`/`checkpoint`/`step`/`parallel`/`pipeline`) fire ONLY inside a
//! `defworkflow` body, so a bare top-level `(parallel …)`/`(checkpoint …)` in an ordinary
//! library file never trips a workflow-only diagnostic. Arities mirror the runtime
//! (`crates/sema-stdlib/src/workflow.rs`) verbatim so static and dynamic never disagree.
//!
//! Diagnostics carry a source span when one is available. The reader keys its `SpanMap` by
//! the `Rc` pointer of each LIST/VECTOR form (map literals and atoms get none), so a
//! diagnostic about a map (bad step opts, the trailing `:status` map) is anchored to its
//! enclosing list form, which always has a span.

use sema_core::{Span, SpanMap, Value};
use std::collections::BTreeMap;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

pub struct Diag {
    pub severity: Severity,
    pub span: Option<Span>,
    pub code: &'static str,
    pub message: String,
    pub hint: Option<String>,
}

impl Diag {
    fn error(span: Option<Span>, code: &'static str, message: impl Into<String>) -> Self {
        Diag {
            severity: Severity::Error,
            span,
            code,
            message: message.into(),
            hint: None,
        }
    }
    fn warn(span: Option<Span>, code: &'static str, message: impl Into<String>) -> Self {
        Diag {
            severity: Severity::Warning,
            span,
            code,
            message: message.into(),
            hint: None,
        }
    }
    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Control-flow heads whose value is produced at runtime — so a workflow body ending in
/// one of these is NOT statically known to (not) return a `{:status …}` map, and we stay
/// silent rather than emit a false `W-NO-STATUS`.
const CONTROL_FLOW_HEADS: &[&str] = &[
    "let",
    "let*",
    "let-values",
    "let*-values",
    "letrec",
    "do",
    "begin",
    "if",
    "when",
    "unless",
    "cond",
    "case",
    "match",
];

/// Statically check one workflow source string. Pure: no I/O, no eval, no LLM. Returns the
/// diagnostics in source order (reader/parse errors first, then semantic findings).
pub fn check_source(src: &str) -> Vec<Diag> {
    // `recover` never bails: a syntax error becomes an entry in `errors` and parsing
    // continues, so one pass surfaces both parse errors AND semantic findings.
    let (forms, spans, _symbol_spans, parse_errors) =
        sema_reader::read_many_with_spans_recover(src);

    let mut diags = Vec::new();
    for e in &parse_errors {
        // Parse failures from the recover-parser are `Reader { message, span }`; anything
        // else degrades to its Display text with no span.
        let (span, message) = match e {
            sema_core::SemaError::Reader { message, span } => (Some(*span), message.clone()),
            other => (None, other.to_string()),
        };
        diags.push(Diag::error(span, "E-PARSE", message));
    }
    for form in &forms {
        find_workflows(form, &spans, &mut diags);
    }
    diags
}

/// Return workflow-declared permission specs from `defworkflow` metadata.
///
/// The metadata key is `:permissions`. Values use the same string syntax as the CLI
/// `--sandbox` flag (`"strict"`, `"no-shell,no-network"`, etc.). Parse errors are
/// left to the normal evaluator path; this helper only inspects forms the
/// recover-parser could build.
pub fn declared_permission_specs(src: &str) -> Result<Vec<String>, String> {
    let (forms, _spans, _symbol_spans, _parse_errors) =
        sema_reader::read_many_with_spans_recover(src);

    let mut specs = Vec::new();
    for form in &forms {
        collect_declared_permission_specs(form, &mut specs)?;
    }
    Ok(specs)
}

fn collect_declared_permission_specs(form: &Value, out: &mut Vec<String>) -> Result<(), String> {
    if head_symbol(form)
        .map(|(head, _)| head == "quote" || head == "quasiquote")
        .unwrap_or(false)
    {
        return Ok(());
    }
    if let Some(items) = list_head(form, "defworkflow") {
        if let Some(meta) = items.get(3).and_then(|v| v.as_map_ref()) {
            if let Some(spec) = permission_spec_from_meta(meta)? {
                out.push(spec);
            }
        }
        return Ok(());
    }
    if let Some(seq) = form.as_seq() {
        for sub in seq {
            collect_declared_permission_specs(sub, out)?;
        }
    }
    Ok(())
}

fn permission_spec_from_meta(meta: &BTreeMap<Value, Value>) -> Result<Option<String>, String> {
    if let Some(value) = meta.get(&Value::keyword("permissions")) {
        return permission_spec_string(":permissions", value).map(Some);
    }
    Ok(None)
}

fn permission_spec_string(key: &str, value: &Value) -> Result<String, String> {
    value
        .as_str()
        .map(String::from)
        .ok_or_else(|| format!("defworkflow {key} must be a sandbox string"))
}

/// Walk the top-level forms looking for `(defworkflow …)` (which may be nested inside a
/// `(do …)` or similar), and check each one. Non-workflow code is left untouched.
fn find_workflows(form: &Value, spans: &SpanMap, out: &mut Vec<Diag>) {
    if let Some(items) = list_head(form, "defworkflow") {
        check_workflow(&items, form, spans, out);
        return;
    }
    if let Some(seq) = form.as_seq() {
        for sub in seq {
            find_workflows(sub, spans, out);
        }
    }
}

/// Check one `(defworkflow name doc meta . body)` form.
fn check_workflow(items: &[Value], form: &Value, spans: &SpanMap, out: &mut Vec<Diag>) {
    let wf_span = span_of(form, spans);

    // (f) shape: name (symbol), doc (string), meta (map), then a body.
    if items.len() < 4 {
        out.push(
            Diag::error(
                wf_span,
                "E-WF-SHAPE",
                "defworkflow needs a name, a doc string, a meta map, and a body",
            )
            .with_hint("(defworkflow name \"doc\" {:phases [...]} body…)"),
        );
        return;
    }
    if items[1].as_symbol().is_none() {
        out.push(Diag::error(
            wf_span,
            "E-WF-NAME",
            "defworkflow name must be a bare symbol",
        ));
    }
    if items[2].as_str().is_none() {
        out.push(Diag::warn(
            wf_span,
            "W-WF-DOC",
            "defworkflow doc should be a string",
        ));
    }
    let meta = items[3].as_map_rc();
    if meta.is_none() {
        out.push(Diag::error(
            wf_span,
            "E-WF-META",
            "defworkflow meta must be a map (e.g. {:phases [...] :budget {...}})",
        ));
    }

    let body = &items[4..];

    // (b) declared :phases vs phases actually opened at the body's top level.
    if let Some(meta) = &meta {
        if let Some(phases_val) = meta.get(&Value::keyword("phases")) {
            let declared: Vec<String> = phases_val
                .as_seq()
                .map(|s| s.iter().filter_map(name_of).collect())
                .unwrap_or_default();
            // Opened phases: top-level (phase "x") markers only — a (phase …) buried in a
            // thunk may run zero or many times, so it is not "definitely opened".
            let opened: Vec<(String, Option<Span>)> = body
                .iter()
                .filter_map(|f| {
                    let it = list_head(f, "phase")?;
                    let label = it.get(1).and_then(|v| v.as_str().map(String::from))?;
                    Some((label, span_of(f, spans)))
                })
                .collect();

            for d in &declared {
                if !opened.iter().any(|(o, _)| o == d) {
                    out.push(Diag::warn(
                        wf_span,
                        "W-PHASE-UNUSED",
                        format!("phase {d:?} is declared in :phases but never opened with (phase {d:?})"),
                    ));
                }
            }
            for (o, sp) in &opened {
                if !declared.iter().any(|d| d == o) {
                    out.push(Diag::warn(
                        *sp,
                        "W-PHASE-UNDECL",
                        format!("phase {o:?} is opened but not declared in :phases (the dashboard won't show it up front)"),
                    ));
                }
            }
        }
    }

    // (c) the body should end in a {:status …} map (the run envelope). Stay silent when
    // the last form is control flow that produces its value at runtime.
    if let Some(last) = body.last() {
        let ends_in_status = last
            .as_map_rc()
            .map(|m| {
                m.keys()
                    .any(|k| k.as_keyword().as_deref() == Some("status"))
            })
            .unwrap_or(false);
        let is_control_flow = last
            .as_seq()
            .and_then(|s| s.first())
            .and_then(name_of_symbol)
            .map(|h| CONTROL_FLOW_HEADS.contains(&h.as_str()))
            .unwrap_or(false);
        if !ends_in_status && !is_control_flow {
            out.push(
                Diag::warn(
                    span_of(last, spans).or(wf_span),
                    "W-NO-STATUS",
                    "workflow body should end in a {:status …} map (the run result envelope)",
                )
                .with_hint("e.g. {:status :success :result …}"),
            );
        }
    }

    // Marker arity/opts checks across the whole body (including nested forms).
    for f in body {
        walk_markers(f, spans, out);
    }
}

/// Recursively check marker arities/opts. Only reached from within a workflow body.
fn walk_markers(form: &Value, spans: &SpanMap, out: &mut Vec<Diag>) {
    if let Some((head, items)) = head_symbol(form) {
        let span = span_of(form, spans);
        match head.as_str() {
            // phase is a ONE-arg marker — the #1 trap. (phase "x" body) is an arity error.
            "phase" => {
                if items.len() != 2 {
                    out.push(
                        Diag::error(
                            span,
                            "E-PHASE-ARITY",
                            format!("phase takes exactly 1 argument (its label), got {}", items.len() - 1),
                        )
                        .with_hint("phase is a marker, not a wrapper: write (phase \"x\") then the body forms as siblings"),
                    );
                } else if items[1].as_str().is_none() {
                    out.push(Diag::warn(
                        span,
                        "W-PHASE-LABEL",
                        "phase label should be a string",
                    ));
                }
            }
            // checkpoint takes 1 or 2 args; the key is a keyword or string.
            "checkpoint" => {
                if !(2..=3).contains(&items.len()) {
                    out.push(Diag::error(
                        span,
                        "E-CKPT-ARITY",
                        format!("checkpoint takes 1 or 2 arguments, got {}", items.len() - 1),
                    ));
                } else if items[1].as_keyword().is_none() && items[1].as_str().is_none() {
                    out.push(Diag::warn(
                        span,
                        "W-CKPT-KEY",
                        "checkpoint key should be a keyword or string",
                    ));
                }
            }
            // step needs at least a prompt; if opts are given, validate the map.
            "step" => {
                if items.len() < 2 {
                    out.push(Diag::error(
                        span,
                        "E-STEP-ARITY",
                        "step needs at least a prompt argument",
                    ));
                } else if let Some(opts) = items.get(2).and_then(|v| v.as_map_rc()) {
                    if let Some(name) = opts.get(&Value::keyword("name")) {
                        if name.as_str().is_none() {
                            out.push(Diag::warn(
                                span,
                                "W-STEP-NAME",
                                "step :name should be a string",
                            ));
                        }
                    }
                    if let Some(tools) = opts.get(&Value::keyword("tools")) {
                        if tools.as_seq().is_none() {
                            out.push(Diag::warn(
                                span,
                                "W-STEP-TOOLS",
                                "step :tools should be a list/vector of tools",
                            ));
                        }
                    }
                    // :agent runs a configured defagent and owns its own tools/model; the
                    // step must not also declare inline :tools/:model (they'd be ignored —
                    // the routing takes the :agent branch). Warn so the author picks one.
                    let has_agent = opts.contains_key(&Value::keyword("agent"));
                    if has_agent {
                        if opts.contains_key(&Value::keyword("tools")) {
                            out.push(Diag::warn(
                                span,
                                "W-STEP-AGENT-TOOLS",
                                "step :agent owns its own tools; inline :tools is ignored",
                            ));
                        }
                        if opts.contains_key(&Value::keyword("model")) {
                            out.push(Diag::warn(
                                span,
                                "W-STEP-AGENT-MODEL",
                                "step :agent owns its own model; inline :model is ignored",
                            ));
                        }
                    }
                }
            }
            // parallel/pipeline are structural — at least one argument beyond the head.
            "parallel" | "pipeline" if items.len() < 2 => {
                out.push(Diag::warn(
                    span,
                    "W-FANOUT-ARITY",
                    format!("{head} expects at least one argument"),
                ));
            }
            _ => {}
        }
    }
    // Recurse into every sub-form so markers inside let/def/pipeline thunks are checked too.
    if let Some(seq) = form.as_seq() {
        for sub in seq {
            walk_markers(sub, spans, out);
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// `(head …)` as a `(head_symbol, items)` pair when `form` is a non-empty list.
fn head_symbol(form: &Value) -> Option<(String, Rc<Vec<Value>>)> {
    let items = form.as_list_rc()?;
    let head = items.first()?.as_symbol()?;
    Some((head, items))
}

/// The items of `form` iff it is a list whose head symbol is exactly `head`.
fn list_head(form: &Value, head: &str) -> Option<Rc<Vec<Value>>> {
    let items = form.as_list_rc()?;
    (items.first()?.as_symbol().as_deref() == Some(head)).then_some(items)
}

/// A keyword-or-string name (phase labels, declared :phases entries).
fn name_of(v: &Value) -> Option<String> {
    v.as_keyword().or_else(|| v.as_str().map(String::from))
}

/// A bare symbol name (control-flow head detection).
fn name_of_symbol(v: &Value) -> Option<String> {
    v.as_symbol()
}

/// The source span of a list/vector form (atoms and maps aren't keyed by the reader, so a
/// caller anchors those to an enclosing list).
fn span_of(form: &Value, spans: &SpanMap) -> Option<Span> {
    let key = form
        .as_list_rc()
        .map(|rc| Rc::as_ptr(&rc) as usize)
        .or_else(|| form.as_vector_rc().map(|rc| Rc::as_ptr(&rc) as usize))?;
    spans.get(&key).copied()
}

// ── reporting ─────────────────────────────────────────────────────────────────

fn counts(diags: &[Diag]) -> (usize, usize) {
    let errors = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    (errors, diags.len() - errors)
}

/// Render the diagnostics to the string the CLI prints — JSON (a machine-readable object)
/// or human-readable lines + a summary. Pure, so it can be asserted on directly.
fn render(file: &str, diags: &[Diag], json: bool) -> String {
    let (errors, warnings) = counts(diags);
    if json {
        let arr: Vec<serde_json::Value> = diags
            .iter()
            .map(|d| {
                serde_json::json!({
                    "severity": d.severity.label(),
                    "code": d.code,
                    "message": d.message,
                    "line": d.span.map(|s| s.line),
                    "col": d.span.map(|s| s.col),
                    "hint": d.hint,
                })
            })
            .collect();
        return serde_json::to_string_pretty(&serde_json::json!({
            "file": file,
            "errors": errors,
            "warnings": warnings,
            "diagnostics": arr,
        }))
        .unwrap_or_else(|_| "{}".into());
    }
    let mut lines = Vec::new();
    for d in diags {
        let loc = match d.span {
            Some(s) => format!("{file}:{}:{}", s.line, s.col),
            None => file.to_string(),
        };
        lines.push(format!(
            "{loc}: {}[{}]: {}",
            d.severity.label(),
            d.code,
            d.message
        ));
        if let Some(hint) = &d.hint {
            lines.push(format!("  hint: {hint}"));
        }
    }
    lines.push(if diags.is_empty() {
        format!("{file}: ok — no issues found")
    } else {
        format!("{file}: {errors} error(s), {warnings} warning(s)")
    });
    lines.join("\n")
}

/// The process exit code: 1 if any error (or, under `strict`, any warning) fired, else 0.
fn exit_code(diags: &[Diag], strict: bool) -> i32 {
    let (errors, warnings) = counts(diags);
    i32::from(errors > 0 || (strict && warnings > 0))
}

/// Print the diagnostics and return the process exit code. `json` emits a machine-readable
/// array. Thin wrapper over [`render`] + [`exit_code`] (both unit-tested).
pub fn report(file: &str, diags: &[Diag], strict: bool, json: bool) -> i32 {
    println!("{}", render(file, diags, json));
    exit_code(diags, strict)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(src: &str) -> Vec<&'static str> {
        check_source(src).into_iter().map(|d| d.code).collect()
    }

    #[test]
    fn clean_workflow_has_no_diagnostics() {
        let src = r#"
            (defworkflow demo "a demo" {:phases ["Inventory" "Audit"]}
              (phase "Inventory")
              (checkpoint :files (list "a" "b"))
              (phase "Audit")
              {:status :success :n (count (checkpoint :files))})
        "#;
        assert!(codes(src).is_empty(), "got {:?}", codes(src));
    }

    #[test]
    fn phase_wrapper_arity_is_the_headline_error() {
        // The #1 trap: (phase "x" body) — phase is a one-arg marker.
        let src = r#"
            (defworkflow demo "d" {:phases ["P"]}
              (phase "P" (checkpoint :x 1))
              {:status :success})
        "#;
        assert!(
            codes(src).contains(&"E-PHASE-ARITY"),
            "got {:?}",
            codes(src)
        );
    }

    #[test]
    fn undeclared_and_unused_phases_warn() {
        let src = r#"
            (defworkflow demo "d" {:phases ["Declared"]}
              (phase "Opened")
              {:status :success})
        "#;
        let c = codes(src);
        assert!(c.contains(&"W-PHASE-UNDECL"), "got {c:?}");
        assert!(c.contains(&"W-PHASE-UNUSED"), "got {c:?}");
    }

    #[test]
    fn missing_status_envelope_warns_but_control_flow_is_silent() {
        let no_status = r#"
            (defworkflow demo "d" {:phases ["P"]}
              (phase "P")
              (checkpoint :x 1))
        "#;
        assert!(
            codes(no_status).contains(&"W-NO-STATUS"),
            "got {:?}",
            codes(no_status)
        );

        // A body ending in control flow is genuinely dynamic — stay silent.
        let control = r#"
            (defworkflow demo "d" {:phases ["P"]}
              (phase "P")
              (if #t {:status :success} {:status :failed}))
        "#;
        assert!(
            !codes(control).contains(&"W-NO-STATUS"),
            "got {:?}",
            codes(control)
        );
    }

    #[test]
    fn checkpoint_arity_and_key_are_checked() {
        let bad_arity = r#"(defworkflow d "d" {} (phase "P") (checkpoint :a 1 2) {:status :ok})"#;
        assert!(
            codes(bad_arity).contains(&"E-CKPT-ARITY"),
            "got {:?}",
            codes(bad_arity)
        );
    }

    #[test]
    fn bare_top_level_markers_outside_a_workflow_are_ignored() {
        // (phase …) with wrong arity in a plain library file is NOT a workflow — no diag.
        let src = r#"(define (phase a b c) (+ a b c)) (phase 1 2 3)"#;
        assert!(codes(src).is_empty(), "got {:?}", codes(src));
    }

    #[test]
    fn syntax_error_surfaces_as_a_parse_diagnostic() {
        // The recover-parser reports the error AND keeps going; we emit E-PARSE.
        let c = codes("(defworkflow d \"x\" {} (phase \"P\")"); // missing close paren
        assert!(c.contains(&"E-PARSE"), "got {c:?}");
    }

    #[test]
    fn defworkflow_shape_is_validated() {
        // < 4 items → E-WF-SHAPE (and nothing crashes on the short form).
        assert!(codes(r#"(defworkflow d "doc")"#).contains(&"E-WF-SHAPE"));
        // non-symbol name, non-string doc, non-map meta each flag.
        let c = codes(r#"(defworkflow "notsym" 42 [:not :map] {:status :ok})"#);
        assert!(c.contains(&"E-WF-NAME"), "got {c:?}");
        assert!(c.contains(&"W-WF-DOC"), "got {c:?}");
        assert!(c.contains(&"E-WF-META"), "got {c:?}");
    }

    #[test]
    fn declared_permission_specs_reads_permissions_key() {
        let specs = declared_permission_specs(
            r#"(defworkflow d "d" {:permissions "no-fs-write"} {:status :ok})"#,
        )
        .unwrap();
        assert_eq!(specs, vec!["no-fs-write"]);
    }

    #[test]
    fn declared_permission_specs_rejects_non_string_permissions() {
        let err = declared_permission_specs(
            r#"(defworkflow d "d" {:permissions [:no-fs-write]} {:status :ok})"#,
        )
        .expect_err("permissions must be a string");
        assert!(err.contains(":permissions"));
    }

    #[test]
    fn declared_permission_specs_ignores_quoted_workflows() {
        let specs = declared_permission_specs(
            r#"
            '(defworkflow quoted "d" {:permissions "all"} {:status :ok})
            `(defworkflow templated "d" {:permissions "all"} {:status :ok})
            (defworkflow actual "d" {:permissions "no-fs-write"} {:status :ok})
            "#,
        )
        .unwrap();
        assert_eq!(specs, vec!["no-fs-write"]);
    }

    #[test]
    fn non_string_phase_label_warns() {
        let c = codes(r#"(defworkflow d "d" {:phases ["P"]} (phase :P) {:status :ok})"#);
        assert!(c.contains(&"W-PHASE-LABEL"), "got {c:?}");
    }

    #[test]
    fn non_keyword_checkpoint_key_warns() {
        let c = codes(r#"(defworkflow d "d" {} (phase "P") (checkpoint 99 1) {:status :ok})"#);
        assert!(c.contains(&"W-CKPT-KEY"), "got {c:?}");
    }

    #[test]
    fn step_arity_and_opts_are_checked() {
        // no prompt → E-STEP-ARITY
        assert!(
            codes(r#"(defworkflow d "d" {} (phase "P") (step) {:status :ok})"#)
                .contains(&"E-STEP-ARITY")
        );
        // bad :name / :tools opts → warnings
        let c = codes(
            r#"(defworkflow d "d" {} (phase "P") (step "go" {:name 1 :tools "x"}) {:status :ok})"#,
        );
        assert!(c.contains(&"W-STEP-NAME"), "got {c:?}");
        assert!(c.contains(&"W-STEP-TOOLS"), "got {c:?}");
    }

    #[test]
    fn step_agent_with_inline_tools_or_model_warns() {
        // :agent owns its own tools/model — declaring inline :tools/:model is a mistake.
        let c = codes(
            r#"(defworkflow d "d" {} (phase "P") (step "go" {:agent a :tools [t] :model "m"}) {:status :ok})"#,
        );
        assert!(c.contains(&"W-STEP-AGENT-TOOLS"), "got {c:?}");
        assert!(c.contains(&"W-STEP-AGENT-MODEL"), "got {c:?}");
    }

    #[test]
    fn empty_fanout_warns() {
        let c = codes(r#"(defworkflow d "d" {} (phase "P") (pipeline) {:status :ok})"#);
        assert!(c.contains(&"W-FANOUT-ARITY"), "got {c:?}");
    }

    #[test]
    fn diagnostics_carry_a_line_and_column() {
        // The phase-arity error must point at the offending (phase …) form's line.
        let src = "(defworkflow d \"d\" {:phases [\"P\"]}\n  (phase \"P\" 1)\n  {:status :ok})";
        let d = check_source(src);
        let phase = d
            .iter()
            .find(|d| d.code == "E-PHASE-ARITY")
            .expect("phase diag");
        let span = phase.span.expect("phase diag has a span");
        assert_eq!(span.line, 2, "the (phase …) form is on line 2");
    }

    // ── render / exit_code ──────────────────────────────────────────────

    fn one_error() -> Vec<Diag> {
        vec![Diag::error(Some(Span::point(2, 3)), "E-TEST", "boom").with_hint("fix it")]
    }
    fn one_warning() -> Vec<Diag> {
        vec![Diag::warn(None, "W-TEST", "careful")]
    }

    #[test]
    fn exit_code_matrix() {
        assert_eq!(exit_code(&[], false), 0, "clean ⇒ 0");
        assert_eq!(exit_code(&one_error(), false), 1, "any error ⇒ 1");
        assert_eq!(
            exit_code(&one_warning(), false),
            0,
            "warning, non-strict ⇒ 0"
        );
        assert_eq!(exit_code(&one_warning(), true), 1, "warning, strict ⇒ 1");
    }

    #[test]
    fn render_human_includes_location_severity_code_hint_and_summary() {
        let out = render("a.sema", &one_error(), false);
        assert!(
            out.contains("a.sema:2:3: error[E-TEST]: boom"),
            "got:\n{out}"
        );
        assert!(out.contains("  hint: fix it"), "got:\n{out}");
        assert!(
            out.contains("a.sema: 1 error(s), 0 warning(s)"),
            "got:\n{out}"
        );
        // A span-less diagnostic anchors to the file with no line:col.
        let w = render("a.sema", &one_warning(), false);
        assert!(w.contains("a.sema: warning[W-TEST]: careful"), "got:\n{w}");
    }

    #[test]
    fn render_human_clean_says_ok() {
        assert!(render("a.sema", &[], false).contains("ok — no issues found"));
    }

    #[test]
    fn render_json_is_valid_and_structured() {
        let out = render("a.sema", &one_error(), true);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(v["file"], "a.sema");
        assert_eq!(v["errors"], 1);
        assert_eq!(v["warnings"], 0);
        let d0 = &v["diagnostics"][0];
        assert_eq!(d0["severity"], "error");
        assert_eq!(d0["code"], "E-TEST");
        assert_eq!(d0["line"], 2);
        assert_eq!(d0["col"], 3);
        assert_eq!(d0["hint"], "fix it");
    }
}
