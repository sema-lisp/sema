//! Run-scoped dynamic context for a workflow run.
//!
//! A workflow run installs a [`WorkflowCtx`] into a thread-local for the duration of
//! the run via [`set_workflow_scope`]; every builtin (`workflow/phase`, `checkpoint`,
//! …) reaches the live context through [`current`]. The scope is restored on drop via
//! a panic-safe RAII guard (mirrors `sema_otel`'s `ConversationGuard`), so a nested
//! run — or a panic unwinding through a phase thunk — cannot leave a stale context
//! installed.
//!
//! The context owns the run's monotonic `seq` counter, the wall-clock seam (`ts` /
//! `dur_ms`, both frozen under `SEMA_WORKFLOW_FIXED_TS` for byte-identical goldens),
//! the append-only [`Journal`], and a Mastra-style checkpoint/state bag.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io;
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use sema_core::Value;

use crate::event::WorkflowEvent;
use crate::journal::Journal;
use crate::RUNS_ROOT;

/// Env var that pins the timestamp string AND forces every `dur_ms` to 0, so the
/// golden `events.jsonl` is byte-identical across runs. When set, its value is used
/// verbatim as the `ts` field of every event (e.g. `SEMA_WORKFLOW_FIXED_TS=0`).
const FIXED_TS_ENV: &str = "SEMA_WORKFLOW_FIXED_TS";

/// Env var that pins the run id (otherwise a process-derived id is generated). Used
/// both to name the run directory and to seed the journal path.
const RUN_ID_ENV: &str = "SEMA_WORKFLOW_RUN_ID";

/// Env var that overrides the run-directory base (the CLI sets it from `--run-dir`).
/// Default is [`RUNS_ROOT`] (`./.sema/runs`).
const RUN_DIR_ENV: &str = "SEMA_WORKFLOW_RUN_DIR";

thread_local! {
    /// The live workflow context, if a run is in progress on this thread. `None`
    /// outside any run; builtins error with a clear message in that case.
    static WORKFLOW: RefCell<Option<Rc<WorkflowCtx>>> = const { RefCell::new(None) };
}

/// Run-scoped dynamic context. Cheap to clone-share via `Rc`; all interior state is
/// `RefCell`/`Cell`, never `&mut self`, so the same `Rc<WorkflowCtx>` handed out by
/// [`current`] can be used while the run is still executing.
pub struct WorkflowCtx {
    /// Stable identifier for this run; names the run dir (`./.sema/runs/<run_id>/`).
    pub run_id: String,
    /// Append-only JSONL journal sink. `RefCell` because `emit` needs `&mut` access
    /// to the underlying writer while the ctx itself is shared `Rc`.
    journal: Rc<RefCell<Journal>>,
    /// Mastra-style run state / checkpoint bag, keyed by the checkpoint name. Doubles
    /// as the `(checkpoint :files)` read-back store for later phases in the same run.
    state: Rc<RefCell<BTreeMap<String, Value>>>,
    /// Monotonic event sequence counter (0-based; first `next_seq()` returns 0).
    seq: Cell<u64>,
    /// Wall-clock origin for `dur_ms`. Ignored when the fixed-ts seam is active.
    start: Instant,
    /// Frozen budget map (optional for Spike 1; carried so the contract is stable).
    budget: BTreeMap<String, Value>,
    /// Cached fixed-ts override (read once at construction). `Some` ⇒ deterministic
    /// seam: `ts()` returns this string and `dur_ms()` returns 0.
    fixed_ts: Option<String>,
}

impl WorkflowCtx {
    /// Build a fresh context for a run.
    ///
    /// `run_id` selection (the caller resolves this, but the helper [`resolve_run_id`]
    /// implements the policy): `SEMA_WORKFLOW_RUN_ID` if set, else a generated id.
    pub fn new(
        run_id: String,
        journal: Journal,
        budget: BTreeMap<String, Value>,
    ) -> Rc<WorkflowCtx> {
        let fixed_ts = std::env::var(FIXED_TS_ENV).ok();
        Rc::new(WorkflowCtx {
            run_id,
            journal: Rc::new(RefCell::new(journal)),
            state: Rc::new(RefCell::new(BTreeMap::new())),
            seq: Cell::new(0),
            start: Instant::now(),
            budget,
            fixed_ts,
        })
    }

    /// Next monotonic sequence number (post-increment: first call yields 0).
    pub fn next_seq(&self) -> u64 {
        let n = self.seq.get();
        self.seq.set(n + 1);
        n
    }

    /// Timestamp for an event. Under the fixed-ts seam this is the verbatim env value
    /// (so goldens are byte-identical); otherwise an RFC3339 UTC instant derived from
    /// `SystemTime` (no `chrono` dependency — this crate only pulls `sema-core` +
    /// `sema-otel` + serde).
    pub fn ts(&self) -> String {
        if let Some(ref fixed) = self.fixed_ts {
            return fixed.clone();
        }
        rfc3339_now()
    }

    /// Milliseconds elapsed since `start`. Always 0 under the fixed-ts seam so the
    /// golden does not depend on real timing.
    pub fn dur_ms(&self) -> u64 {
        if self.fixed_ts.is_some() {
            return 0;
        }
        self.start.elapsed().as_millis() as u64
    }

    /// Append one event to the journal. Write errors are swallowed by the journal
    /// (same trust model as the OTel file exporter); journaling never aborts the run.
    pub fn emit(&self, event: WorkflowEvent) {
        self.journal.borrow_mut().write(&event);
    }

    /// This run's stable identifier (also the run-dir name).
    pub fn run_id(&self) -> String {
        self.run_id.clone()
    }

    /// Store a checkpoint / run-state value under `key`, replacing any prior value.
    pub fn checkpoint_set(&self, key: &str, val: Value) {
        self.state.borrow_mut().insert(key.to_string(), val);
    }

    /// Read a checkpoint / run-state value. `None` if the key was never set in this run.
    pub fn checkpoint_get(&self, key: &str) -> Option<Value> {
        self.state.borrow().get(key).cloned()
    }

    /// Builtin-facing alias of [`Self::checkpoint_set`].
    pub fn store_checkpoint(&self, key: &str, val: Value) {
        self.checkpoint_set(key, val);
    }

    /// Builtin-facing alias of [`Self::checkpoint_get`].
    pub fn read_checkpoint(&self, key: &str) -> Option<Value> {
        self.checkpoint_get(key)
    }

    /// Opaque, lossy digest of a checkpoint value for the event stream: the md5 hex
    /// of the value's lossy-JSON encoding. Lossy is acceptable in Spike 1 (the digest
    /// is for journal compactness/diffing, not resume — that's Spike 4's canonical
    /// encoder). Stable for a given value within a process.
    pub fn value_digest(&self, v: &Value) -> String {
        let json = sema_core::json::value_to_json_lossy(v);
        let bytes = serde_json::to_vec(&json).unwrap_or_default();
        format!("{:x}", md5::compute(bytes))
    }

    /// Write the final `{:status …}` envelope to `result.json` (best-effort; a write
    /// failure is swallowed like a journal write).
    pub fn write_result(&self, envelope: &Value) {
        let json = sema_core::json::value_to_json_lossy(envelope);
        let _ = self.journal.borrow().write_result(&json);
    }

    /// The frozen budget map for this run (empty when none was supplied).
    pub fn budget(&self) -> &BTreeMap<String, Value> {
        &self.budget
    }

    /// Best-effort flush of the journal writer (e.g. at `run.ended`).
    pub fn flush(&self) {
        self.journal.borrow_mut().flush();
    }
}

/// Panic-safe RAII guard that restores the PREVIOUS workflow context on drop. Copied
/// from `sema_otel::ConversationGuard`'s shape: the restore happens in `Drop`, so an
/// `Err` short-circuit OR a panic unwinding through the run body both reinstall the
/// prior context (supporting nested runs and never leaking a stale `Rc`).
pub struct WorkflowGuard {
    prev: Option<Rc<WorkflowCtx>>,
}

impl Drop for WorkflowGuard {
    fn drop(&mut self) {
        WORKFLOW.with(|c| *c.borrow_mut() = self.prev.take());
    }
}

/// Low-level: install an already-built `ctx` as the live workflow context for the
/// current thread, returning a guard whose drop restores whatever was installed
/// before. Used by unit tests and by [`set_workflow_scope`].
pub fn install_scope(ctx: Rc<WorkflowCtx>) -> WorkflowGuard {
    let prev = WORKFLOW.with(|c| c.borrow_mut().replace(ctx));
    WorkflowGuard { prev }
}

/// High-level entry the `workflow/run` builtin calls: resolve the run id + run-dir,
/// open the journal, build the `WorkflowCtx`, write `metadata.json`, install the
/// scope, and return the guard. The journal-open error propagates so the runtime can
/// fail the run cleanly (per-event writes below are best-effort).
///
/// `meta` is the workflow's metadata map (`{:args … :budget … :perms …}`); Spike 1
/// records it into `metadata.json` but does not enforce `:budget`/`:perms`.
pub fn set_workflow_scope(name: &str, doc: &str, meta: &Value) -> io::Result<WorkflowGuard> {
    let run_id = resolve_run_id();
    let runs_root = resolve_runs_root();
    let journal = Journal::open(&runs_root, &run_id)?;
    // metadata.json — self-describing run header. Best-effort; not part of the
    // byte-identical events.jsonl oracle.
    let metadata = serde_json::json!({
        "workflow": name,
        "doc": doc,
        "run_id": run_id,
        "code_version": "",
        "meta": sema_core::json::value_to_json_lossy(meta),
    });
    let _ = journal.write_metadata(&metadata);
    // Spike 1 carries an empty budget map (the :budget cap is not enforced yet).
    let ctx = WorkflowCtx::new(run_id, journal, BTreeMap::new());
    Ok(install_scope(ctx))
}

/// Resolve the run-directory base: the `SEMA_WORKFLOW_RUN_DIR` seam (set by the CLI
/// `--run-dir`) if present, else the project-local [`RUNS_ROOT`].
pub fn resolve_runs_root() -> String {
    std::env::var(RUN_DIR_ENV).unwrap_or_else(|_| RUNS_ROOT.to_string())
}

/// The live workflow context, if a run is in progress on this thread.
pub fn current() -> Option<Rc<WorkflowCtx>> {
    WORKFLOW.with(|c| c.borrow().clone())
}

/// Resolve the run id for a new run: `SEMA_WORKFLOW_RUN_ID` if set, else a generated
/// id of the form `wf_<unix_secs>_<pid>` — deterministic-enough to be unique per
/// process without a random-number dependency, and overridable to a fixed value in
/// tests (the golden oracle sets `SEMA_WORKFLOW_RUN_ID=wf_test_0001`).
pub fn resolve_run_id() -> String {
    if let Ok(id) = std::env::var(RUN_ID_ENV) {
        if !id.is_empty() {
            return id;
        }
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("wf_{secs}_{}", std::process::id())
}

/// Format `SystemTime::now()` as an RFC3339 / ISO-8601 UTC string (`YYYY-MM-DDTHH:MM:SSZ`)
/// without pulling in `chrono`. Civil-date conversion via the standard
/// days-since-epoch algorithm (Howard Hinnant's `civil_from_days`).
fn rfc3339_now() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, min, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Convert a count of days since 1970-01-01 to a (year, month, day) civil date.
/// Hinnant's algorithm; valid for the full SystemTime range we will ever journal.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_is_monotonic_from_zero() {
        let ctx = WorkflowCtx::new("wf_t".into(), Journal::null(), BTreeMap::new());
        assert_eq!(ctx.next_seq(), 0);
        assert_eq!(ctx.next_seq(), 1);
        assert_eq!(ctx.next_seq(), 2);
    }

    #[test]
    fn fixed_ts_freezes_ts_and_dur() {
        std::env::set_var(FIXED_TS_ENV, "1970-01-01T00:00:00Z");
        let ctx = WorkflowCtx::new("wf_t".into(), Journal::null(), BTreeMap::new());
        assert_eq!(ctx.ts(), "1970-01-01T00:00:00Z");
        assert_eq!(ctx.dur_ms(), 0);
        std::env::remove_var(FIXED_TS_ENV);
    }

    #[test]
    fn checkpoint_round_trips() {
        let ctx = WorkflowCtx::new("wf_t".into(), Journal::null(), BTreeMap::new());
        assert_eq!(ctx.checkpoint_get("files"), None);
        ctx.checkpoint_set("files", Value::int(3));
        assert_eq!(ctx.checkpoint_get("files"), Some(Value::int(3)));
    }

    #[test]
    fn scope_restores_previous_on_drop() {
        assert!(current().is_none());
        let outer = WorkflowCtx::new("outer".into(), Journal::null(), BTreeMap::new());
        let g_outer = install_scope(outer.clone());
        assert_eq!(
            current().map(|c| c.run_id.clone()).as_deref(),
            Some("outer")
        );
        {
            let inner = WorkflowCtx::new("inner".into(), Journal::null(), BTreeMap::new());
            let _g_inner = install_scope(inner);
            assert_eq!(
                current().map(|c| c.run_id.clone()).as_deref(),
                Some("inner")
            );
        }
        // inner guard dropped → outer reinstated
        assert_eq!(
            current().map(|c| c.run_id.clone()).as_deref(),
            Some("outer")
        );
        drop(g_outer);
        assert!(current().is_none());
    }

    #[test]
    fn civil_date_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2026-06-24 is 20628 days after epoch.
        assert_eq!(civil_from_days(20_628), (2026, 6, 24));
    }
}
