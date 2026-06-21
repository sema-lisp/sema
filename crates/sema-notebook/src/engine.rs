//! Notebook evaluation engine.
//!
//! Manages a persistent Sema interpreter environment and evaluates cells
//! sequentially. Each cell shares the same environment, so definitions
//! in earlier cells are visible to later ones.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use chrono::Utc;
use sema_core::{pretty_print, resolve, Spur, Value};
use sema_eval::Interpreter;

use crate::format::{CellOutput, CellType, Notebook, OutputType};

/// Default wall-clock budget for a single cell evaluation.
/// Override via the `SEMA_NOTEBOOK_TIMEOUT_MS` environment variable; set the
/// variable to `0` to disable the timeout entirely.
pub const DEFAULT_CELL_TIMEOUT_MS: u64 = 30_000;

/// Resolve the configured cell evaluation timeout. Reads
/// `SEMA_NOTEBOOK_TIMEOUT_MS` (milliseconds). Returns `None` to disable.
fn resolve_cell_timeout() -> Option<Duration> {
    match std::env::var("SEMA_NOTEBOOK_TIMEOUT_MS") {
        Ok(s) => match s.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(ms) => Some(Duration::from_millis(ms)),
            Err(_) => Some(Duration::from_millis(DEFAULT_CELL_TIMEOUT_MS)),
        },
        Err(_) => Some(Duration::from_millis(DEFAULT_CELL_TIMEOUT_MS)),
    }
}

/// Snapshot of the global environment bindings before a cell eval.
struct EnvSnapshot {
    /// Cloned bindings from the global env.
    bindings: Vec<(Spur, Value)>,
    /// The cell that was about to be evaluated.
    cell_id: String,
    /// The cell's outputs before evaluation (for restore).
    cell_outputs: Vec<CellOutput>,
    /// IDs of downstream code cells that were freshly transitioned to `stale=true`
    /// by this evaluation (i.e. they were `stale == false` with non-empty outputs
    /// before `mark_downstream_stale` ran). Undo restores them to `stale = false`.
    downstream_stale_ids: Vec<String>,
}

/// Result of evaluating a single cell.
#[derive(Debug, Clone)]
pub struct EvalResult {
    /// The output to store in the cell.
    pub output: CellOutput,
    /// Captured stdout from println/display/print calls.
    pub stdout: String,
}

/// Result of undoing the last cell evaluation.
#[derive(Debug)]
pub struct UndoInfo {
    /// The cell whose evaluation was undone.
    pub cell_id: String,
}

/// The notebook evaluation engine. Wraps an `Interpreter` and a `Notebook`.
pub struct Engine {
    /// The Sema interpreter with persistent environment.
    pub interpreter: Interpreter,
    /// The notebook being worked on.
    pub notebook: Notebook,
    /// Snapshot from before the last cell eval (for single-step undo).
    snapshot: Option<EnvSnapshot>,
    /// Per-cell wall-clock evaluation budget. `None` disables the limit.
    cell_timeout: Option<Duration>,
}

impl Engine {
    /// Create a new engine with a fresh interpreter and notebook.
    pub fn new(notebook: Notebook) -> Self {
        let interpreter = Interpreter::new();
        Self {
            interpreter,
            notebook,
            snapshot: None,
            cell_timeout: resolve_cell_timeout(),
        }
    }

    /// Create an engine from an existing notebook file path.
    pub fn from_file(path: &std::path::Path) -> Result<Self, String> {
        let notebook = Notebook::load(path)?;
        Ok(Self::new(notebook))
    }

    /// Override the per-cell wall-clock evaluation budget. Pass `None` to disable.
    pub fn set_cell_timeout(&mut self, timeout: Option<Duration>) {
        self.cell_timeout = timeout;
    }

    /// The currently configured per-cell evaluation budget.
    pub fn cell_timeout(&self) -> Option<Duration> {
        self.cell_timeout
    }

    /// Evaluate a single cell by ID. Returns the result and updates the notebook.
    ///
    /// Automatically snapshots the environment before evaluation so
    /// the cell can be undone with [`undo_last_cell`].
    pub fn eval_cell(&mut self, cell_id: &str) -> Result<EvalResult, String> {
        let idx = self
            .notebook
            .cell_index(cell_id)
            .ok_or_else(|| format!("Cell not found: {cell_id}"))?;

        let cell = &self.notebook.cells[idx];
        if cell.cell_type != CellType::Code {
            return Err("Cannot evaluate a markdown cell".to_string());
        }

        // Snapshot env + cell outputs before evaluation.
        let mut bindings = Vec::new();
        self.interpreter
            .global_env
            .iter_bindings(|spur, value| bindings.push((spur, value.clone())));
        // Record the downstream code cells that will be flipped to stale by
        // `mark_downstream_stale` below — those are the ones currently
        // `stale == false` with non-empty outputs.
        let downstream_stale_ids: Vec<String> = self.notebook.cells[idx + 1..]
            .iter()
            .filter(|c| c.cell_type == CellType::Code && !c.stale && !c.outputs.is_empty())
            .map(|c| c.id.clone())
            .collect();
        self.snapshot = Some(EnvSnapshot {
            bindings,
            cell_id: cell_id.to_string(),
            cell_outputs: cell.outputs.clone(),
            downstream_stale_ids,
        });

        let source = cell.source.clone();
        // INTERNAL span per evaluated cell. Nests under the `notebook.run_all` root
        // when invoked from eval_all; otherwise it's a standalone one-cell trace.
        // LLM/tool spans emitted during the cell nest beneath it via the TL stack.
        let result = {
            let _cell_span = sema_otel::vm_span(&format!("notebook.cell {cell_id}"));
            self.eval_source(&source)
        };

        // Mark downstream cells as stale
        self.notebook.mark_downstream_stale(idx);

        // Update the cell with outputs: stdout first (if any), then value/error
        let cell = &mut self.notebook.cells[idx];
        cell.outputs.clear();
        if !result.stdout.is_empty() {
            cell.outputs.push(CellOutput {
                output_type: OutputType::Stdout,
                display: result.stdout.clone(),
                sema_value: None,
                timestamp: Utc::now(),
                cost_usd: None,
                requires_reeval: false,
                duration_ms: None,
            });
        }
        cell.outputs.push(result.output.clone());
        cell.stale = false;

        Ok(result)
    }

    /// Evaluate raw source code in the notebook's environment.
    ///
    /// Captures stdout during evaluation so that `println`/`display`/`print`
    /// output is available in the result rather than going to the server's
    /// terminal.
    pub fn eval_source(&mut self, source: &str) -> EvalResult {
        let start = Instant::now();

        // Apply the wall-clock deadline to the interpreter context so an
        // infinite loop in a cell does not hang the engine thread forever.
        // The deadline is cleared after the eval regardless of outcome.
        if let Some(budget) = self.cell_timeout {
            self.interpreter
                .ctx
                .set_eval_deadline(Some(Instant::now() + budget));
        } else {
            self.interpreter.ctx.set_eval_deadline(None);
        }

        // Capture stdout during evaluation via the thread-local output hook
        // rather than redirecting the process stdout fd. Hooks are per-thread,
        // so concurrent cell evaluations on different engine threads don't
        // contend for a single global fd redirect, and program output can never
        // leak into a server's protocol stream on the real stdout.
        let buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink = buf.clone();
        sema_core::set_stdout_hook(Some(Box::new(move |s: &str| {
            if let Ok(mut b) = sink.lock() {
                b.push_str(s);
            }
        })));
        let eval_result = self.interpreter.eval_str_compiled(source);
        sema_core::set_stdout_hook(None);
        let captured = buf.lock().map(|b| b.clone()).unwrap_or_default();

        // Always clear the deadline so subsequent cells (and unrelated
        // interpreter usage) are not poisoned by it.
        self.interpreter.ctx.set_eval_deadline(None);

        let duration_ms = start.elapsed().as_millis() as u64;

        match eval_result {
            Ok(value) => {
                let display = format_value_for_display(&value);
                let sema_value = value_to_sexp(&value);

                EvalResult {
                    stdout: captured,
                    output: CellOutput {
                        output_type: OutputType::Value,
                        display,
                        sema_value: Some(sema_value),
                        timestamp: Utc::now(),
                        cost_usd: None,
                        requires_reeval: is_opaque(&value),
                        duration_ms: Some(duration_ms),
                    },
                }
            }
            Err(err) => EvalResult {
                stdout: captured,
                output: CellOutput {
                    output_type: OutputType::Error,
                    display: format_error(&err),
                    sema_value: None,
                    timestamp: Utc::now(),
                    cost_usd: None,
                    requires_reeval: false,
                    duration_ms: Some(duration_ms),
                },
            },
        }
    }

    /// Evaluate all code cells in order.
    pub fn eval_all(&mut self) -> Vec<(String, Result<EvalResult, String>)> {
        let cell_ids: Vec<String> = self
            .notebook
            .cells
            .iter()
            .filter(|c| c.cell_type == CellType::Code)
            .map(|c| c.id.clone())
            .collect();

        // One root trace per "Run All"; each cell becomes a child span.
        let _root = sema_otel::vm_span("notebook.run_all");
        cell_ids
            .into_iter()
            .map(|id| {
                let result = self.eval_cell(&id);
                (id, result)
            })
            .collect()
    }

    /// Evaluate specific cells by index (1-based).
    pub fn eval_cells(&mut self, indices: &[usize]) -> Vec<(String, Result<EvalResult, String>)> {
        let cell_ids: Vec<String> = indices
            .iter()
            .filter_map(|&i| {
                self.notebook
                    .cells
                    .get(i.saturating_sub(1))
                    .map(|c| c.id.clone())
            })
            .collect();

        cell_ids
            .into_iter()
            .map(|id| {
                let result = self.eval_cell(&id);
                (id, result)
            })
            .collect()
    }

    /// Create a new code cell, evaluate it, and return the result.
    pub fn create_and_eval(&mut self, source: &str) -> Result<(String, EvalResult), String> {
        let id = self.notebook.add_code_cell(source);
        let result = self.eval_cell(&id)?;
        Ok((id, result))
    }

    /// Get all current environment bindings as a map of name -> display string.
    pub fn env_bindings(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        self.interpreter.global_env.iter_bindings(|spur, value| {
            let name = resolve(spur);
            let display = format_value_for_display(value);
            map.insert(name, display);
        });
        map
    }

    /// Whether the last cell evaluation can be undone.
    pub fn can_undo(&self) -> bool {
        self.snapshot.is_some()
    }

    /// Undo the last cell evaluation, restoring the environment and cell outputs.
    pub fn undo_last_cell(&mut self) -> Result<UndoInfo, String> {
        let snapshot = self
            .snapshot
            .take()
            .ok_or_else(|| "Nothing to undo".to_string())?;

        // Restore environment bindings
        self.interpreter
            .global_env
            .replace_bindings(snapshot.bindings);

        // Restore the cell's outputs
        if let Some(cell) = self.notebook.cell_mut(&snapshot.cell_id) {
            cell.outputs = snapshot.cell_outputs;
        }

        // Revert the downstream `stale` flags that this evaluation flipped on.
        for id in &snapshot.downstream_stale_ids {
            if let Some(cell) = self.notebook.cell_mut(id) {
                cell.stale = false;
            }
        }

        Ok(UndoInfo {
            cell_id: snapshot.cell_id,
        })
    }

    /// Reset the interpreter and clear all cell outputs.
    pub fn reset(&mut self) {
        self.interpreter = Interpreter::new();
        self.snapshot = None;
        for cell in &mut self.notebook.cells {
            cell.outputs.clear();
            cell.stale = false;
        }
    }
}

/// Format a Sema value for human-readable display in the notebook.
pub fn format_value_for_display(value: &Value) -> String {
    if value.is_nil() {
        return String::new();
    }
    pretty_print(value, 80)
}

/// Convert a value to its S-expression string form for persistence.
pub fn value_to_sexp(value: &Value) -> String {
    format!("{value}")
}

/// Check if a value is opaque (cannot be round-tripped via read).
fn is_opaque(value: &Value) -> bool {
    use sema_core::ValueView;
    matches!(
        value.view(),
        ValueView::NativeFn(_)
            | ValueView::Lambda(_)
            | ValueView::Macro(_)
            | ValueView::Stream(_)
            | ValueView::Thunk(_)
    )
}

/// Format a SemaError for display in the notebook.
pub fn format_error(err: &sema_core::SemaError) -> String {
    format!("{err}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Notebook;

    fn test_engine() -> Engine {
        Engine::new(Notebook::new("Test"))
    }

    #[test]
    fn eval_simple_expression() {
        let mut engine = test_engine();
        let (id, result) = engine.create_and_eval("(+ 1 2)").unwrap();
        assert_eq!(result.output.output_type, OutputType::Value);
        assert_eq!(result.output.display, "3");
        assert!(result.output.duration_ms.is_some());
        // Cell should be stored in notebook with at least the value output
        assert!(engine.notebook.cell(&id).is_some());
        let outputs = &engine.notebook.cell(&id).unwrap().outputs;
        assert!(!outputs.is_empty());
        assert!(outputs.iter().any(|o| o.output_type == OutputType::Value));
    }

    #[test]
    fn definitions_persist_across_cells() {
        let mut engine = test_engine();
        engine.create_and_eval("(define x 42)").unwrap();
        let (_, result) = engine.create_and_eval("x").unwrap();
        assert_eq!(result.output.display, "42");
    }

    #[test]
    fn eval_error_produces_error_output() {
        let mut engine = test_engine();
        let (_, result) = engine.create_and_eval("(undefined-fn)").unwrap();
        assert_eq!(result.output.output_type, OutputType::Error);
        assert!(!result.output.display.is_empty());
    }

    #[test]
    fn eval_cell_not_found() {
        let mut engine = test_engine();
        let result = engine.eval_cell("nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cell not found"));
    }

    #[test]
    fn eval_markdown_cell_rejected() {
        let mut engine = test_engine();
        let id = engine.notebook.add_markdown_cell("# Hello");
        let result = engine.eval_cell(&id);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("markdown"));
    }

    #[test]
    fn eval_all_runs_all_code_cells() {
        let mut engine = test_engine();
        engine.notebook.add_code_cell("(define a 1)");
        engine.notebook.add_markdown_cell("text");
        engine.notebook.add_code_cell("(+ a 2)");

        let results = engine.eval_all();
        assert_eq!(results.len(), 2); // only code cells
        assert!(results[0].1.is_ok());
        assert!(results[1].1.is_ok());
        assert_eq!(results[1].1.as_ref().unwrap().output.display, "3");
    }

    #[test]
    fn eval_marks_downstream_stale() {
        let mut engine = test_engine();
        let id1 = engine.notebook.add_code_cell("(define x 1)");
        let id2 = engine.notebook.add_code_cell("x");

        // Eval both cells
        engine.eval_cell(&id1).unwrap();
        engine.eval_cell(&id2).unwrap();
        assert!(!engine.notebook.cell(&id2).unwrap().stale);

        // Re-eval cell 1 should mark cell 2 as stale
        engine.eval_cell(&id1).unwrap();
        assert!(engine.notebook.cell(&id2).unwrap().stale);
    }

    #[test]
    fn reset_clears_state() {
        let mut engine = test_engine();
        engine.create_and_eval("(define x 1)").unwrap();
        assert!(!engine.notebook.cells.is_empty());

        engine.reset();
        // Cells remain but outputs are cleared
        assert!(!engine.notebook.cells.is_empty());
        assert!(engine.notebook.cells[0].outputs.is_empty());

        // x should be unbound after reset
        let (_, result) = engine.create_and_eval("x").unwrap();
        assert_eq!(result.output.output_type, OutputType::Error);
    }

    #[test]
    fn nil_displays_as_empty() {
        assert_eq!(format_value_for_display(&Value::nil()), "");
    }

    #[test]
    fn opaque_values_require_reeval() {
        let mut engine = test_engine();
        let (_, result) = engine.create_and_eval("(fn (x) x)").unwrap();
        assert!(result.output.requires_reeval);
    }

    #[test]
    fn non_opaque_values_do_not_require_reeval() {
        let mut engine = test_engine();
        let (_, result) = engine.create_and_eval("42").unwrap();
        assert!(!result.output.requires_reeval);
    }

    // ── Undo tests ──────────────────────────────────────────────

    #[test]
    fn undo_restores_env_after_define() {
        let mut engine = test_engine();
        engine.create_and_eval("(define x 42)").unwrap();
        engine.undo_last_cell().unwrap();

        // x should be unbound now
        let (_, result) = engine.create_and_eval("x").unwrap();
        assert_eq!(result.output.output_type, OutputType::Error);
    }

    #[test]
    fn undo_restores_env_after_set_bang() {
        let mut engine = test_engine();
        engine.create_and_eval("(define x 1)").unwrap();

        // Overwrite x — this creates a new snapshot
        let id2 = engine.notebook.add_code_cell("(set! x 999)");
        engine.eval_cell(&id2).unwrap();

        // Undo the set! — x should be back to 1
        engine.undo_last_cell().unwrap();
        let (_, result) = engine.create_and_eval("x").unwrap();
        assert_eq!(result.output.display, "1");
    }

    #[test]
    fn undo_clears_errored_cell_output() {
        let mut engine = test_engine();
        let (id, _) = engine.create_and_eval("(bad-fn)").unwrap();
        assert!(!engine.notebook.cell(&id).unwrap().outputs.is_empty());

        engine.undo_last_cell().unwrap();
        assert!(engine.notebook.cell(&id).unwrap().outputs.is_empty());
    }

    #[test]
    fn undo_when_nothing_to_undo_errors() {
        let mut engine = test_engine();
        let result = engine.undo_last_cell();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Nothing to undo"));
    }

    #[test]
    fn undo_only_available_once() {
        let mut engine = test_engine();
        engine.create_and_eval("(define x 1)").unwrap();
        engine.undo_last_cell().unwrap();

        // Second undo should fail
        let result = engine.undo_last_cell();
        assert!(result.is_err());
    }

    #[test]
    fn new_eval_replaces_snapshot() {
        let mut engine = test_engine();
        engine.create_and_eval("(define a 1)").unwrap();
        engine.create_and_eval("(define b 2)").unwrap();

        // Undo should restore state before (define b 2), not before (define a 1)
        engine.undo_last_cell().unwrap();
        let (_, result) = engine.create_and_eval("a").unwrap();
        assert_eq!(result.output.display, "1"); // a is still defined

        // But b should be gone (after another undo for the eval we just did)
        engine.undo_last_cell().unwrap();
        let (_, result) = engine.create_and_eval("b").unwrap();
        assert_eq!(result.output.output_type, OutputType::Error);
    }

    #[test]
    fn can_undo_reflects_state() {
        let mut engine = test_engine();
        assert!(!engine.can_undo());

        engine.create_and_eval("(+ 1 2)").unwrap();
        assert!(engine.can_undo());

        engine.undo_last_cell().unwrap();
        assert!(!engine.can_undo());
    }

    // ── Infinite-loop / timeout tests ───────────────────────────

    #[test]
    fn infinite_recursion_aborts_within_budget() {
        let mut engine = test_engine();
        engine.set_cell_timeout(Some(Duration::from_millis(500)));

        let start = Instant::now();
        let (_, result) = engine
            .create_and_eval("(define (loop) (loop)) (loop)")
            .unwrap();
        let elapsed = start.elapsed();

        assert_eq!(
            result.output.output_type,
            OutputType::Error,
            "infinite recursion should produce an Error output, got {:?}",
            result.output
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "eval should abort well before 2s; took {elapsed:?}"
        );
        assert!(
            result.output.display.to_lowercase().contains("time budget")
                || result
                    .output
                    .display
                    .to_lowercase()
                    .contains("infinite loop"),
            "error message should mention the time budget / infinite loop; got: {}",
            result.output.display
        );
    }

    #[test]
    fn infinite_while_loop_aborts_within_budget() {
        let mut engine = test_engine();
        engine.set_cell_timeout(Some(Duration::from_millis(500)));

        let start = Instant::now();
        let (_, result) = engine.create_and_eval("(while #t 1)").unwrap();
        let elapsed = start.elapsed();

        assert_eq!(result.output.output_type, OutputType::Error);
        assert!(
            elapsed < Duration::from_secs(2),
            "while-loop eval should abort well before 2s; took {elapsed:?}"
        );
    }

    #[test]
    fn engine_recovers_after_timeout() {
        let mut engine = test_engine();
        engine.set_cell_timeout(Some(Duration::from_millis(300)));

        // First cell hangs and should time out
        let (_, hang) = engine
            .create_and_eval("(define (loop) (loop)) (loop)")
            .unwrap();
        assert_eq!(hang.output.output_type, OutputType::Error);

        // Engine must still be usable for subsequent cells
        let (_, ok) = engine.create_and_eval("(+ 1 2)").unwrap();
        assert_eq!(ok.output.output_type, OutputType::Value);
        assert_eq!(ok.output.display, "3");

        // And undo should still work for the most recent cell
        assert!(engine.can_undo());
        engine.undo_last_cell().unwrap();
    }

    #[test]
    fn undo_reverts_downstream_stale() {
        let mut engine = test_engine();
        let id_a = engine.notebook.add_code_cell("(define x 1)");
        let id_b = engine.notebook.add_code_cell("(* x 10)");

        // Evaluate both cells.
        engine.eval_cell(&id_a).unwrap();
        engine.eval_cell(&id_b).unwrap();
        assert!(!engine.notebook.cell(&id_b).unwrap().stale);

        // Edit cell A and re-evaluate — cell B should be marked stale.
        engine.notebook.cell_mut(&id_a).unwrap().source = "(define x 2)".to_string();
        engine.eval_cell(&id_a).unwrap();
        assert!(
            engine.notebook.cell(&id_b).unwrap().stale,
            "B should be stale after re-evaluating A"
        );

        // Undo the re-eval of A — B should no longer be stale.
        engine.undo_last_cell().unwrap();
        assert!(
            !engine.notebook.cell(&id_b).unwrap().stale,
            "undo must clear the stale flag it set on downstream cells"
        );
    }

    #[test]
    fn reset_clears_undo() {
        let mut engine = test_engine();
        engine.create_and_eval("(define x 1)").unwrap();
        assert!(engine.can_undo());

        engine.reset();
        assert!(!engine.can_undo());
        assert!(engine.undo_last_cell().is_err());
    }
}
