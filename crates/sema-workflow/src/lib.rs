//! Sema dynamic-workflow runtime: an in-process runtime that journals a frozen JSONL
//! run-directory and returns a discriminated-union `{:status …}` envelope.
//!
//! The body runs sequentially; leaves fan out with bounded concurrency (`parallel` /
//! `pipeline`). `--resume` short-circuits any agent/checkpoint leaf already recorded in
//! the run's `memo/` sidecar dir. The run directory is the stable public contract (treat
//! like the `.semac` bytecode format) — its layout and event vocabulary are FROZEN
//! (append-only Option/skippable fields only).
//!
//! This crate is a leaf: it depends only on `sema-core` + `sema-otel` + serde.
//! The builtins that invoke Sema thunks (`workflow/run`, `workflow/phase`,
//! `checkpoint`, `workflow/agent`) live in `sema-stdlib`, which depends on this crate.

pub mod context;
pub mod event;
mod journal;

pub use context::{
    current_for, cur_agent_for, resolve_run_id, set_cur_agent_for, set_workflow_scope, WorkflowCtx,
    WorkflowGuard, WorkflowTaskState,
};
pub use event::WorkflowEvent;
pub use journal::Journal;

/// Project-local run-directory root (cwd-relative, git-ignorable). NOT
/// `sema_home()` (`~/.sema`) — a run dir belongs to the project being worked on.
/// The CLI overrides the base via the `SEMA_WORKFLOW_RUN_DIR` seam (see
/// [`context::resolve_runs_root`]).
pub const RUNS_ROOT: &str = ".sema/runs";

/// Filename of the cross-run SQLite projection index, under the run-dir base
/// (`<run-dir>/index.db`). One DB indexes every run for the dashboard's cross-run views.
pub const INDEX_DB: &str = "index.db";
