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
use std::collections::{BTreeMap, HashMap};
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
    /// Parsed spend caps (absent ⇒ that dimension is unenforced). `usd` is best-effort
    /// (depends on the pricing table); `tokens` is deterministic from usage.
    cost_limit: Option<f64>,
    token_limit: Option<u64>,
    /// Running totals charged from each agent leaf's usage. Single-thread `Cell` is
    /// sound (the VM + scheduler are cooperative single-thread); under a concurrent
    /// fan-out the per-leaf attribution is BEST-EFFORT (the `LAST_USAGE` thread-local
    /// the snapshot reads is not swapped per task), but the cap still trips reliably.
    cost_spent: Cell<f64>,
    tokens_spent: Cell<u64>,
    /// Sticky "a cap was exceeded" latch. Set by [`Self::charge`] once a total passes
    /// its cap; checked at agent ENTRY (to refuse launching further leaves) and by
    /// `workflow/run` after the body (to force a `:failed` envelope). A latch — not
    /// `Err` propagation — because the `__fanout-tagged` engine swallows a leaf `Err`
    /// into `nil`, so an exception can't stop a concurrent batch.
    over_budget: Cell<bool>,
    /// `start_seq` of the currently-open phase (the phase.started event's seq), so
    /// checkpoints/agents/budget events can be attributed to their phase.
    cur_phase_seq: Cell<Option<u64>>,
    /// Label of the currently-open phase, paired with `cur_phase_seq`. Marker-style
    /// phases need the label to emit the matching `phase.ended` when the next marker
    /// (or the run end) closes the phase.
    cur_phase_label: RefCell<Option<String>>,
    /// Per-name agent invocation counter, for minting unique `agent_id`s.
    agent_n: RefCell<BTreeMap<String, u64>>,
    /// The `agent_id` of the agent currently executing (set by `workflow/agent`), so
    /// `workflow/tool-call` can attribute a tool call to it. `None` outside an agent.
    cur_agent_id: RefCell<Option<String>>,
    /// Resume state. `resuming` ⇒ this run was launched with `--resume`, so leaves whose
    /// content-key is in `resume_memos` short-circuit (return the recorded value, skip
    /// the model + events). `resume_memos` is loaded from the prior run's `memo/` dir at
    /// scope open. `code_version` is folded into every content-key, so a changed workflow
    /// (different version) simply produces different keys ⇒ no memo hits ⇒ full re-run
    /// (automatic invalidation, no guard file). `key_seen` mints a per-base occurrence
    /// ordinal so identical-prompt repeats in source order line up across runs.
    resuming: Cell<bool>,
    code_version: RefCell<String>,
    resume_memos: RefCell<HashMap<String, Value>>,
    key_seen: RefCell<HashMap<String, u32>>,
    /// The run's `--args` JSON string (for the run.started event). Empty if none.
    args_json: String,
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
        Self::new_with_args(run_id, journal, budget, String::new())
    }

    /// As [`Self::new`], plus the run's `--args` JSON string for `run.started`.
    pub fn new_with_args(
        run_id: String,
        journal: Journal,
        budget: BTreeMap<String, Value>,
        args_json: String,
    ) -> Rc<WorkflowCtx> {
        let fixed_ts = std::env::var(FIXED_TS_ENV).ok();
        // Parse spend caps from the budget submap (tolerate an int usd, e.g. `:usd 2`).
        let cost_limit = budget
            .get("usd")
            .and_then(|v| v.as_float().or_else(|| v.as_int().map(|i| i as f64)));
        // Tolerate an int OR a float token cap (`:tokens 5` or `:tokens 5.0`), so a
        // float never silently drops the cap.
        let token_limit = budget
            .get("tokens")
            .and_then(|v| v.as_int().or_else(|| v.as_float().map(|f| f as i64)))
            .map(|i| i as u64);
        Rc::new(WorkflowCtx {
            run_id,
            journal: Rc::new(RefCell::new(journal)),
            state: Rc::new(RefCell::new(BTreeMap::new())),
            seq: Cell::new(0),
            start: Instant::now(),
            cost_limit,
            token_limit,
            cost_spent: Cell::new(0.0),
            tokens_spent: Cell::new(0),
            over_budget: Cell::new(false),
            cur_phase_seq: Cell::new(None),
            cur_phase_label: RefCell::new(None),
            agent_n: RefCell::new(BTreeMap::new()),
            cur_agent_id: RefCell::new(None),
            resuming: Cell::new(false),
            code_version: RefCell::new(String::new()),
            resume_memos: RefCell::new(HashMap::new()),
            key_seen: RefCell::new(HashMap::new()),
            args_json,
            fixed_ts,
        })
    }

    /// The run's `--args` JSON string (empty if none).
    pub fn args_json(&self) -> &str {
        &self.args_json
    }

    /// Open a marker-style phase: record its `phase.started` seq AND label so the next
    /// marker (or the run end) can emit the matching `phase.ended`. Subsequent
    /// checkpoints / agents / budget events attribute to `start_seq`.
    pub fn open_phase(&self, start_seq: u64, label: String) {
        self.cur_phase_seq.set(Some(start_seq));
        *self.cur_phase_label.borrow_mut() = Some(label);
    }

    /// Close the currently-open phase, returning its `(start_seq, label)` so the caller
    /// can emit `phase.ended`. Clears the open-phase tracking; returns `None` when no
    /// phase is open (e.g. a workflow with no `(phase …)` markers).
    pub fn take_open_phase(&self) -> Option<(u64, String)> {
        let label = self.cur_phase_label.borrow_mut().take()?;
        let seq = self.cur_phase_seq.replace(None);
        seq.map(|s| (s, label))
    }

    /// `start_seq` of the open phase, if any.
    pub fn phase_seq(&self) -> Option<u64> {
        self.cur_phase_seq.get()
    }

    /// Mint a unique `agent_id` for an agent of role `name` (`<name>_<n>`, 1-based).
    pub fn next_agent_id(&self, name: &str) -> String {
        let mut m = self.agent_n.borrow_mut();
        let n = m.entry(name.to_string()).or_insert(0);
        *n += 1;
        format!("{name}_{n}")
    }

    /// Set (or clear) the agent currently executing, so `workflow/tool-call` can
    /// attribute to it. Set on `workflow/agent` entry, cleared on exit.
    pub fn set_cur_agent(&self, agent_id: Option<String>) {
        *self.cur_agent_id.borrow_mut() = agent_id;
    }

    /// The `agent_id` of the executing agent, if inside one.
    pub fn cur_agent(&self) -> Option<String> {
        self.cur_agent_id.borrow().clone()
    }

    /// A stable short resume key for a checkpoint (`ck_<hex>` over key + digest).
    pub fn content_key(&self, key: &str, value_digest: &str) -> String {
        let h = format!(
            "{:x}",
            md5::compute(format!("{key}:{value_digest}").as_bytes())
        );
        format!("ck_{}", &h[..8])
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

    /// True under the fixed-timestamp test seam (`SEMA_WORKFLOW_FIXED_TS`). Callers
    /// that measure their own per-leaf durations force them to 0 in this mode so
    /// goldens stay byte-identical.
    pub fn deterministic(&self) -> bool {
        self.fixed_ts.is_some()
    }

    /// This run's stable identifier (also the run-dir name).
    pub fn run_id(&self) -> String {
        self.run_id.clone()
    }

    /// Store a checkpoint / run-state value under `key`, replacing any prior value.
    pub fn store_checkpoint(&self, key: &str, val: Value) {
        self.state.borrow_mut().insert(key.to_string(), val);
    }

    /// Read a checkpoint / run-state value. `None` if the key was never set in this run.
    pub fn read_checkpoint(&self, key: &str) -> Option<Value> {
        self.state.borrow().get(key).cloned()
    }

    /// Opaque, lossy digest of a checkpoint value for the event stream: the md5 hex
    /// of the value's lossy-JSON encoding. The digest is for journal compactness and
    /// diffing — NOT resume identity (resume keys on the input-derived content-key and
    /// stores the real value in `memo/`, round-trip-guarded). Stable within a process.
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

    /// True when a `:budget` cap (usd and/or tokens) is in force for this run.
    pub fn has_budget(&self) -> bool {
        self.cost_limit.is_some() || self.token_limit.is_some()
    }

    /// The token cap, for the `budget_limit` field of a `Budget` event (typed `u64`).
    /// `None` for a usd-only budget (the event field is tokens; usd has no slot).
    pub fn budget_limit_for_event(&self) -> Option<u64> {
        self.token_limit
    }

    /// Add one agent leaf's usage to the running totals and, if either cap is now
    /// exceeded, set the sticky [`Self::over_budget`] latch. Returns `true` once the
    /// run is over budget. Charge AFTER the leaf's events are journaled, so the leaf
    /// that tips the cap is itself fully recorded; the NEXT leaf is the one refused.
    pub fn charge(&self, cost: Option<f64>, tokens: u64) -> bool {
        if let Some(c) = cost {
            self.cost_spent.set(self.cost_spent.get() + c);
        }
        self.tokens_spent.set(self.tokens_spent.get() + tokens);
        let over = self
            .cost_limit
            .is_some_and(|lim| self.cost_spent.get() > lim)
            || self
                .token_limit
                .is_some_and(|lim| self.tokens_spent.get() > lim);
        if over {
            self.over_budget.set(true);
        }
        over
    }

    /// Whether a cap has been exceeded this run (the sticky latch).
    pub fn over_budget(&self) -> bool {
        self.over_budget.get()
    }

    // ── Resume / content-key memoization ──────────────────────────────────────

    /// Set the workflow's code version (folded into every content-key). A changed
    /// workflow ⇒ different version ⇒ different keys ⇒ no memo hits ⇒ full re-run.
    pub fn set_code_version(&self, v: String) {
        *self.code_version.borrow_mut() = v;
    }

    /// Enter resume mode with the prior run's memos (content-key → value).
    pub fn enter_resume(&self, memos: HashMap<String, Value>) {
        self.resuming.set(true);
        *self.resume_memos.borrow_mut() = memos;
    }

    /// True when this run is a `--resume` continuation.
    pub fn resuming(&self) -> bool {
        self.resuming.get()
    }

    /// The label of the currently-open phase (empty outside any phase). Part of a
    /// content-key so the same leaf in different phases keys distinctly.
    pub fn cur_phase_label(&self) -> String {
        self.cur_phase_label.borrow().clone().unwrap_or_default()
    }

    /// Next 0-based occurrence ordinal for a content-key base, so identical-input leaves
    /// repeated in body order get distinct keys that line up across runs (deterministic
    /// for a sequential body; best-effort under a concurrent fan-out).
    fn next_occurrence(&self, base: &str) -> u32 {
        let mut m = self.key_seen.borrow_mut();
        let n = m.entry(base.to_string()).or_insert(0);
        let cur = *n;
        *n += 1;
        cur
    }

    /// Content-key for an agent leaf: a stable hash over (kind, code-version, phase,
    /// name, prompt, schema-repr) plus an occurrence ordinal. Length-prefixed so
    /// `("a","bc")` and `("ab","c")` never collide.
    pub fn agent_content_key(
        &self,
        prompt: &str,
        schema_repr: &str,
        name: &str,
        phase: &str,
    ) -> String {
        let cv = self.code_version.borrow().clone();
        let base = hash_fields(&["agent", &cv, phase, name, prompt, schema_repr]);
        format!("{base}_{}", self.next_occurrence(&base))
    }

    /// Content-key for a checkpoint write: hash over (kind, code-version, phase, key)
    /// plus an occurrence ordinal.
    pub fn checkpoint_content_key(&self, key: &str, phase: &str) -> String {
        let cv = self.code_version.borrow().clone();
        let base = hash_fields(&["checkpoint", &cv, phase, key]);
        format!("{base}_{}", self.next_occurrence(&base))
    }

    /// Look up a memoized value by content-key (only meaningful while `resuming`).
    pub fn memo_lookup(&self, content_key: &str) -> Option<Value> {
        self.resume_memos.borrow().get(content_key).cloned()
    }

    /// Persist a leaf's value as a memo sidecar AND into the in-run map — but ONLY if it
    /// round-trips through JSON identically (`value_to_json_lossy`→`json_to_value` is
    /// lossy for keyword/string keys, records, typed arrays). A value that doesn't
    /// survive is left un-memoized, so it re-runs on resume rather than resuming wrong.
    /// Must be called OUTSIDE any held journal borrow.
    pub fn memo_store(&self, content_key: &str, v: &Value) {
        let json = sema_core::json::value_to_json_lossy(v);
        if sema_core::json::json_to_value(&json) == *v {
            let _ = self.journal.borrow().write_memo(content_key, &json);
            self.resume_memos
                .borrow_mut()
                .insert(content_key.to_string(), v.clone());
        }
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
/// `meta` is the workflow's metadata map (`{:phases … :budget … :args …}`); it is
/// recorded into `metadata.json`, and `:budget` is parsed into the run's spend caps
/// (`:perms` is not yet enforced).
pub fn set_workflow_scope(name: &str, doc: &str, meta: &Value) -> io::Result<WorkflowGuard> {
    let run_id = resolve_run_id();
    let runs_root = resolve_runs_root();
    let code_version = std::env::var(CODE_VERSION_ENV).unwrap_or_default();
    // Resume mode: reuse the run dir, write a fresh sibling events.resume-<n>.jsonl
    // segment (keeps the frozen first-line/seq invariants), and preload prior memos.
    let resuming = std::env::var(RESUME_ENV).map(|v| v == "1").unwrap_or(false);
    let journal = if resuming {
        let seg = crate::journal::next_resume_segment(&runs_root, &run_id);
        Journal::open_named(&runs_root, &run_id, &seg)?
    } else {
        Journal::open(&runs_root, &run_id)?
    };
    // metadata.json — self-describing run header. Best-effort; not part of the
    // byte-identical events.jsonl oracle.
    let metadata = serde_json::json!({
        "workflow": name,
        "doc": doc,
        "run_id": run_id,
        "code_version": code_version,
        "meta": sema_core::json::value_to_json_lossy(meta),
    });
    let _ = journal.write_metadata(&metadata);
    // The `:budget` submap of meta becomes the run's enforced spend caps.
    // The CLI sets SEMA_WORKFLOW_ARGS_JSON to the verbatim `--args` string.
    let args_json = std::env::var("SEMA_WORKFLOW_ARGS_JSON").unwrap_or_default();
    let ctx = WorkflowCtx::new_with_args(run_id.clone(), journal, parse_budget(meta), args_json);
    ctx.set_code_version(code_version);
    if resuming {
        let memos: HashMap<String, Value> = crate::journal::load_memos(&runs_root, &run_id)
            .into_iter()
            .map(|(ck, json)| (ck, sema_core::json::json_to_value(&json)))
            .collect();
        ctx.enter_resume(memos);
    }
    Ok(install_scope(ctx))
}

/// Extract the `:budget` submap from a workflow `meta` map, flattening its keyword
/// keys (`:usd`, `:tokens`) to the `String` keys [`WorkflowCtx`] parses. Returns an
/// empty map when there is no (or a malformed) `:budget` — caps stay unenforced, never
/// a panic.
pub fn parse_budget(meta: &Value) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    if let Some(m) = meta.as_map_rc() {
        if let Some(b) = m.get(&Value::keyword("budget")).and_then(|v| v.as_map_rc()) {
            for (k, v) in b.iter() {
                if let Some(name) = k.as_keyword() {
                    out.insert(name, v.clone());
                }
            }
        }
    }
    out
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

/// Length-prefixed md5 over a field list → short hex. Length-prefixing each field
/// (`u64` LE length then bytes) means concatenation ambiguities like `("a","bc")` vs
/// `("ab","c")` produce different digests — the separator-collision fix.
fn hash_fields(fields: &[&str]) -> String {
    let mut buf = Vec::new();
    for f in fields {
        buf.extend_from_slice(&(f.len() as u64).to_le_bytes());
        buf.extend_from_slice(f.as_bytes());
    }
    let h = format!("{:x}", md5::compute(&buf));
    h[..16].to_string()
}

/// Env seam: set to "1" by the CLI `--resume` path to enter resume mode.
const RESUME_ENV: &str = "SEMA_WORKFLOW_RESUME";
/// Env seam: a stable hash of the workflow source, folded into every content-key.
const CODE_VERSION_ENV: &str = "SEMA_WORKFLOW_CODE_VERSION";

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

    fn ctx_with_budget(pairs: &[(&str, Value)]) -> Rc<WorkflowCtx> {
        let mut b = BTreeMap::new();
        for (k, v) in pairs {
            b.insert(k.to_string(), v.clone());
        }
        WorkflowCtx::new_with_args("wf_t".into(), Journal::null(), b, String::new())
    }

    #[test]
    fn charge_trips_usd_cap_and_latches() {
        let ctx = ctx_with_budget(&[("usd", Value::float(0.01))]);
        assert!(!ctx.charge(Some(0.005), 10), "under cap must not trip");
        assert!(!ctx.over_budget());
        assert!(ctx.charge(Some(0.02), 100), "crossing cap trips");
        assert!(ctx.over_budget(), "latch is sticky");
        // Once latched, it stays latched even on a tiny later charge.
        let _ = ctx.charge(Some(0.0), 0);
        assert!(ctx.over_budget());
    }

    #[test]
    fn charge_enforces_tokens_when_cost_unknown() {
        let ctx = ctx_with_budget(&[("tokens", Value::int(50))]);
        assert!(!ctx.charge(None, 40), "cost None still counts tokens");
        assert!(!ctx.over_budget());
        assert!(ctx.charge(None, 20), "60 > 50 trips on tokens alone");
        assert!(ctx.over_budget());
        assert_eq!(ctx.budget_limit_for_event(), Some(50));
    }

    #[test]
    fn no_budget_never_trips() {
        let ctx = WorkflowCtx::new("wf_t".into(), Journal::null(), BTreeMap::new());
        assert!(!ctx.has_budget());
        assert!(!ctx.charge(Some(9999.0), 9_999_999));
        assert!(!ctx.over_budget());
        assert_eq!(ctx.budget_limit_for_event(), None);
    }

    #[test]
    fn parse_budget_extracts_caps_and_tolerates_absence() {
        let mut bm = BTreeMap::new();
        bm.insert(Value::keyword("usd"), Value::float(2.5));
        bm.insert(Value::keyword("tokens"), Value::int(1000));
        let mut meta = BTreeMap::new();
        meta.insert(Value::keyword("budget"), Value::map(bm));
        let parsed = parse_budget(&Value::map(meta));
        assert_eq!(parsed.get("usd").and_then(|v| v.as_float()), Some(2.5));
        assert_eq!(parsed.get("tokens").and_then(|v| v.as_int()), Some(1000));
        // No :budget at all → empty (unenforced).
        assert!(parse_budget(&Value::map(BTreeMap::new())).is_empty());
    }

    #[test]
    fn content_keys_are_stable_distinct_and_length_prefixed() {
        let ctx = WorkflowCtx::new("wf_t".into(), Journal::null(), BTreeMap::new());
        ctx.set_code_version("v1".into());
        // First occurrence of each distinct input is stable; differing inputs differ.
        let k_a = ctx.agent_content_key("audit a.php", "[:list :string]", "auditor", "Audit");
        let k_b = ctx.agent_content_key("audit b.php", "[:list :string]", "auditor", "Audit");
        assert_ne!(k_a, k_b, "different prompts ⇒ different keys");
        // Length-prefixing: ('a','bc') must not collide with ('ab','c').
        let k1 = ctx.agent_content_key("a", "bc", "n", "p");
        let k2 = ctx.agent_content_key("ab", "c", "n", "p");
        assert_ne!(
            k1, k2,
            "length-prefixed fields can't collide via concatenation"
        );
        // Occurrence ordinal: a repeated identical leaf gets a distinct key.
        let r1 = ctx.checkpoint_content_key("files", "Inventory");
        let r2 = ctx.checkpoint_content_key("files", "Inventory");
        assert_ne!(
            r1, r2,
            "repeated identical checkpoint ⇒ distinct occurrence key"
        );
    }

    #[test]
    fn code_version_changes_invalidate_keys() {
        let ctx1 = WorkflowCtx::new("a".into(), Journal::null(), BTreeMap::new());
        ctx1.set_code_version("v1".into());
        let ctx2 = WorkflowCtx::new("b".into(), Journal::null(), BTreeMap::new());
        ctx2.set_code_version("v2".into());
        assert_ne!(
            ctx1.agent_content_key("p", "s", "n", "ph"),
            ctx2.agent_content_key("p", "s", "n", "ph"),
            "a changed code-version produces different content-keys (auto-invalidation)"
        );
    }

    #[test]
    fn memo_store_round_trip_guard_skips_unsurvivable_values() {
        let ctx = WorkflowCtx::new("wf_t".into(), Journal::null(), BTreeMap::new());
        // A plain string round-trips → memoized and looked up.
        ctx.memo_store("ck_text", &Value::string("hello"));
        assert_eq!(ctx.memo_lookup("ck_text"), Some(Value::string("hello")));
        // A keyword-keyed map round-trips (json_to_value rebuilds keyword keys).
        let mut m = BTreeMap::new();
        m.insert(Value::keyword("body"), Value::string("x"));
        let kw_map = Value::map(m);
        ctx.memo_store("ck_map", &kw_map);
        assert_eq!(ctx.memo_lookup("ck_map"), Some(kw_map));
        // A map with a NON-string/keyword key does NOT survive JSON round-trip (the int
        // key becomes a string), so the guard leaves it un-memoized → it re-runs on
        // resume rather than resuming a different value. This exercises the FALSE branch.
        let mut bad = BTreeMap::new();
        bad.insert(Value::int(1), Value::int(2));
        ctx.memo_store("ck_bad", &Value::map(bad));
        assert_eq!(
            ctx.memo_lookup("ck_bad"),
            None,
            "a non-round-trippable value must be left un-memoized"
        );
    }

    #[test]
    fn checkpoint_round_trips() {
        let ctx = WorkflowCtx::new("wf_t".into(), Journal::null(), BTreeMap::new());
        assert_eq!(ctx.read_checkpoint("files"), None);
        ctx.store_checkpoint("files", Value::int(3));
        assert_eq!(ctx.read_checkpoint("files"), Some(Value::int(3)));
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
