//! Notebook evaluation engine.
//!
//! Manages a persistent Sema interpreter environment and evaluates cells
//! sequentially. Each cell shares the same environment, so definitions
//! in earlier cells are visible to later ones.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use sema_core::runtime::RootId;
use sema_core::{pretty_print, resolve, Spur, Value};
use sema_eval::Interpreter;
use sema_vm::runtime::{OutputEvent, RootOptions, RuntimeCommandHandle};

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

/// A `Send + Sync` token that lets another thread (a server request handler,
/// a watchdog) cancel whichever cell an [`Engine`] is *currently* driving,
/// without needing access to the (`!Send`) `Engine`/`Interpreter` itself.
///
/// Obtained once via [`Engine::cancel_token`] and cloned freely — cloning is
/// cheap (an `Rc`-free channel handle plus a shared `Arc<Mutex<..>>>` root
/// id). `cancel_running` is a no-op (`false`) when no cell is currently
/// being driven, or once the running root has already settled.
#[derive(Clone)]
pub struct CancelToken {
    command: RuntimeCommandHandle,
    running_root: Arc<Mutex<Option<RootId>>>,
}

impl CancelToken {
    /// Cancel the cell currently being driven, if any. Returns `false` if no
    /// cell is running right now, or if the underlying runtime is already
    /// gone — in neither case is that an error, just nothing to cancel.
    pub fn cancel_running(&self) -> bool {
        let root = *self
            .running_root
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        match root {
            Some(root) => self.command.cancel_root(root),
            None => false,
        }
    }
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
    /// The root id of the cell currently being driven, if any. Set just
    /// before [`Interpreter::drive_until_settled`] and cleared right after,
    /// so a [`CancelToken`] cloned out via [`Engine::cancel_token`] can
    /// target the in-flight root from another thread.
    running_root: Arc<Mutex<Option<RootId>>>,
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
            running_root: Arc::new(Mutex::new(None)),
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

    /// A cloneable, `Send + Sync` token another thread can use to cancel
    /// whichever cell this engine is currently driving (`async/sleep 60000`
    /// stuck in a loop, a runaway agent call). See [`CancelToken`].
    pub fn cancel_token(&self) -> CancelToken {
        CancelToken {
            command: self.interpreter.command_handle(),
            running_root: self.running_root.clone(),
        }
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
            self.eval_source_named(&source, Some(cell_id.to_string()))
        };

        // Cell-eval safe point (CORE-2, plan §5.2 point b): the kernel is
        // long-lived and each cell can leave garbage cycles (recursive local
        // closures, data cycles) plus the just-replaced undo snapshot's
        // released bindings. Threshold-gated; pins skip descent into the live
        // kernel namespace.
        if sema_core::gc_should_collect() {
            let pins = sema_core::gc_env_chain_pins(&self.interpreter.global_env);
            sema_core::gc_threshold_collect(&pins, sema_core::GcTrigger::NotebookCell);
        }

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
        self.eval_source_named(source, None)
    }

    /// Same as [`eval_source`](Self::eval_source), tagging the submitted
    /// root with `name` (the cell id, when called from [`eval_cell`]) for
    /// host-side observability.
    ///
    /// Drives the source through the public host API — `submit_str` +
    /// `drive_until_settled` + `take_output` — rather than a private
    /// eval entry point, so cell evaluation is cancellable like any other
    /// host consumer of `Interpreter`: [`running_root`](Self::running_root)
    /// (via [`cancel_token`](Self::cancel_token)) is set to the submitted
    /// root's id for the duration of the drive.
    ///
    /// `submit_str` compiles against `self.interpreter.global_env` — the
    /// same global env every other eval entry point on this interpreter
    /// uses — so cell-to-cell `define` sharing is unaffected by this
    /// routing.
    fn eval_source_named(&mut self, source: &str, name: Option<String>) -> EvalResult {
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

        // Submit with `capture_output: true` so the cell's
        // `println`/`display`/`print` output is routed into root-tagged
        // `OutputEvent`s (drained below) instead of the real process
        // stdout — the fd-free capture the old thread-local stdout hook
        // provided, now attributed per-root by the runtime itself.
        let opts = RootOptions {
            name,
            capture_output: true,
        };
        let (eval_result, captured) = match self.interpreter.submit_str(source, opts) {
            Ok(handle) => {
                *self
                    .running_root
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = Some(handle.id());
                let result = self.interpreter.drive_until_settled(&handle);
                *self
                    .running_root
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = None;

                // Only this root's own output belongs to this cell — a
                // detached task left over from an earlier cell that happens
                // to print while this root drives is captured too (never
                // leaked to the real stdout), but attributed to its own
                // root, not this cell. Stderr (`println-error` etc.) is
                // folded into the same text stream as stdout, in the order
                // events were emitted, since the `.sema-nb` format has a
                // single text output per cell (`OutputType::Stdout`) rather
                // than a distinct stderr channel — the sink preserves
                // per-root FIFO/execution order, so simple interleaving here
                // reproduces the order the cell actually printed in.
                let captured: String = self
                    .interpreter
                    .take_output()
                    .into_iter()
                    .filter_map(|event| match event {
                        OutputEvent::Stdout { root, text } if root == handle.id() => Some(text),
                        OutputEvent::Stderr { root, text } if root == handle.id() => Some(text),
                        _ => None,
                    })
                    .collect();
                (result, captured)
            }
            Err(err) => (Err(err), String::new()),
        };

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

    /// Reset the interpreter and clear all cell outputs. The old kernel's
    /// memory is actually returned: its whole env⇄closure graph is reclaimed
    /// by the cycle collector before this returns (CORE-2 — the shape-E
    /// teardown leak used to pin ~168 KB per reset forever).
    pub fn reset(&mut self) {
        // Drop the undo snapshot FIRST: it holds clones of every old global
        // binding, and each clone is an external strong count that would
        // (correctly) keep the dying env graph alive through the teardown
        // collection run by `Interpreter::drop`.
        self.snapshot = None;
        self.interpreter = Interpreter::new();
        // Mop-up pass for anything the teardown collection could not prove
        // dead at drop time (candidates stay registered, so late-released
        // cycles are still discoverable). Pins: the fresh kernel's namespace.
        sema_core::gc_collect(
            &sema_core::gc_env_chain_pins(&self.interpreter.global_env),
            sema_core::GcTrigger::NotebookReset,
        );
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

    /// CORE-2: `reset` must actually RETURN the old kernel's memory, not just
    /// rebind a fresh interpreter next to an immortal env⇄closure graph. The
    /// oracle is a `Weak` on the old global env's bindings allocation:
    /// strong-count 0 after reset means the whole graph (defines, recursive
    /// local closures, the data-only channel cycle, the undo snapshot's
    /// clones) was reclaimed. Deterministic — no timing, no thresholds: reset
    /// runs an explicit full collection.
    #[test]
    fn reset_returns_old_kernel_memory() {
        let mut engine = test_engine();
        // Populate every CORE-2 leak shape: a top-level define (shape E), a
        // recursive local closure kept alive in a global (shape U), and a
        // closure-free channel self-cycle (data shape).
        engine
            .create_and_eval(
                "(begin
                   (define (mk)
                     (define (r n) (if (<= n 0) 0 (r (- n 1))))
                     r)
                   (define keep (mk))
                   (define ch (channel/new 2))
                   (channel/send ch (list ch))
                   (keep 3))",
            )
            .unwrap();
        // A second cell so the undo snapshot (taken BEFORE each eval) holds
        // clones of the cycle bindings above — reset must release the
        // snapshot before the teardown collection or those clones pin the
        // dying graph as external references.
        engine.create_and_eval("(keep 2)").unwrap();
        let weak_bindings = std::rc::Rc::downgrade(&engine.interpreter.global_env.bindings);

        engine.reset();

        assert_eq!(
            weak_bindings.strong_count(),
            0,
            "reset must reclaim the old kernel's entire env graph"
        );
        // The fresh kernel works and is unpolluted.
        let (_, result) = engine.create_and_eval("(+ 20 22)").unwrap();
        assert_eq!(result.output.display, "42");
        let (_, result) = engine.create_and_eval("keep").unwrap();
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

    // ── Host API migration tests (P6-1 Task 5) ─────────────────────

    /// Multi-line `println` output from a single cell lands in that cell's
    /// captured `stdout`, in the order it was printed — the root-tagged
    /// `take_output` draining must preserve per-root FIFO order.
    #[test]
    fn multiline_output_lands_in_order() {
        let mut engine = test_engine();
        let (_, result) = engine
            .create_and_eval(r#"(println "one") (println "two") (println "three")"#)
            .unwrap();

        let lines: Vec<&str> = result.stdout.lines().collect();
        assert_eq!(
            lines,
            vec!["one", "two", "three"],
            "expected the three prints in order, got: {:?}",
            result.stdout
        );
    }

    /// `println-error` output (stderr) must not be dropped — it belongs in
    /// the cell's captured output alongside stdout, interleaved in the
    /// order the events were emitted. The `.sema-nb` format has a single
    /// text stream per cell (`OutputType::Stdout`), so stderr lines are
    /// folded into that same stream rather than inventing a new output
    /// variant.
    #[test]
    fn stderr_output_is_not_dropped() {
        let mut engine = test_engine();
        let (_, result) = engine
            .create_and_eval(r#"(println "out1") (println-error "err1") (println "out2")"#)
            .unwrap();

        let lines: Vec<&str> = result.stdout.lines().collect();
        assert_eq!(
            lines,
            vec!["out1", "err1", "out2"],
            "expected stdout/stderr interleaved in emission order, got: {:?}",
            result.stdout
        );
    }

    /// Cancelling a running cell from another thread (via `CancelToken`,
    /// itself backed by `Interpreter::command_handle`) settles the cell
    /// promptly as an Error output instead of running out the full sleep,
    /// and the engine is still usable for the next cell afterward.
    #[test]
    fn cancel_running_cell_from_another_thread() {
        let mut engine = test_engine();
        engine.set_cell_timeout(None); // isolate cancellation from the timeout path
        let token = engine.cancel_token();

        let canceller = std::thread::spawn(move || {
            // Poll until the cell's root is actually being driven, then
            // cancel it. Bounded: the test would fail on timeout below if
            // this loop never found a running root.
            for _ in 0..2000 {
                if token.cancel_running() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            panic!("never observed a running root to cancel");
        });

        let start = Instant::now();
        let (_, result) = engine.create_and_eval("(async/sleep 60000)").unwrap();
        let elapsed = start.elapsed();
        canceller.join().unwrap();

        assert_eq!(
            result.output.output_type,
            OutputType::Error,
            "cancelled cell should settle as an Error output, got {:?}",
            result.output
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "cancellation should settle the cell well before the 60s sleep; took {elapsed:?}"
        );

        // The engine must survive: the next cell evaluates normally.
        let (_, ok) = engine.create_and_eval("(+ 1 2)").unwrap();
        assert_eq!(ok.output.output_type, OutputType::Value);
        assert_eq!(ok.output.display, "3");
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
