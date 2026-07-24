//! Shared in-process workflow test harness for the FakeProvider-driven workflow tests
//! (budget / tools / resume). Consolidates the SERIAL guard + `SEMA_WORKFLOW_*` env
//! setup + journal read-back that was copy-pasted across the three files.
//!
//! Each integration-test BINARY that does `mod workflow_common;` gets its own copy of
//! `SERIAL` — which is exactly right: binaries are separate processes (env never leaks
//! across them), and the lock just serializes the workflow tests WITHIN one binary
//! (they share the process-global `SEMA_WORKFLOW_*` env and the thread-local provider
//! registry).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::{FakeProvider, FakeRecorder};

static SERIAL: Mutex<()> = Mutex::new(());

/// Options for a single workflow run. `run_dir` is caller-owned so a resume can reuse
/// the same directory across two `run_workflow` calls.
pub struct RunOpts<'a> {
    pub run_id: &'a str,
    pub run_dir: &'a Path,
    pub resume: bool,
    pub code_version: &'a str,
    pub args_json: &'a str,
}

impl<'a> RunOpts<'a> {
    /// A plain fresh run into `run_dir/<run_id>/` (no resume, empty code version).
    pub fn fresh(run_id: &'a str, run_dir: &'a Path) -> Self {
        Self {
            run_id,
            run_dir,
            resume: false,
            code_version: "",
            args_json: "{}",
        }
    }
}

pub struct RunOutput {
    /// Events of the file written THIS run (events.jsonl, or events.resume-1.jsonl on
    /// resume).
    pub events: Vec<serde_json::Value>,
    /// The final `result.json` envelope (`Null` if absent).
    pub result: serde_json::Value,
    /// The provider recorder, for `call_count()` etc.
    pub recorder: Arc<FakeRecorder>,
}

/// Run `src` as a workflow under the fixed-ts seam against `fake`, into
/// `opts.run_dir/<run_id>/`. Serialized via a process-wide lock. Reads back the events
/// file written THIS run plus `result.json`.
pub fn run_workflow(src: &str, fake: FakeProvider, opts: RunOpts) -> RunOutput {
    let _g: MutexGuard<()> = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

    std::env::set_var("SEMA_WORKFLOW_FIXED_TS", "0");
    std::env::set_var("SEMA_WORKFLOW_RUN_ID", opts.run_id);
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", opts.run_dir);
    std::env::set_var("SEMA_WORKFLOW_CODE_VERSION", opts.code_version);
    std::env::set_var("SEMA_WORKFLOW_ARGS_JSON", opts.args_json);
    if opts.resume {
        std::env::set_var("SEMA_WORKFLOW_RESUME", "1");
    } else {
        std::env::remove_var("SEMA_WORKFLOW_RESUME");
    }

    let interp = Interpreter::new();
    reset_runtime_state();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    let _ = interp.eval_str_compiled(src);

    for v in [
        "SEMA_WORKFLOW_FIXED_TS",
        "SEMA_WORKFLOW_RUN_ID",
        "SEMA_WORKFLOW_RUN_DIR",
        "SEMA_WORKFLOW_CODE_VERSION",
        "SEMA_WORKFLOW_ARGS_JSON",
        "SEMA_WORKFLOW_RESUME",
    ] {
        std::env::remove_var(v);
    }

    let run = opts.run_dir.join(opts.run_id);
    let events_file = if opts.resume {
        run.join("events.resume-1.jsonl")
    } else {
        run.join("events.jsonl")
    };
    let events = std::fs::read_to_string(&events_file)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid event json"))
        .collect();
    let result = std::fs::read_to_string(run.join("result.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    RunOutput {
        events,
        result,
        recorder,
    }
}

/// Like [`run_once`] but WITHOUT pinning an explicit `SEMA_WORKFLOW_RUN_ID`, so every
/// `workflow/run` (including one a `defworkflow`/`workflow/run-form` nests inside the
/// outer run) gets its OWN generated id and directory. Under the A2 run-identity guarantee
/// two fresh runs can never share a dir, so a nested/emitted run only coexists with its
/// parent when ids are generated — the production shape (a plain `sema workflow run` never
/// pins an id; only `--resume` does). Reads back the run whose `run.started` names
/// `workflow_name` (there may be several run dirs — the outer and any nested runs).
pub fn run_once_generated(src: &str, fake: FakeProvider, workflow_name: &str) -> RunOutput {
    let _g: MutexGuard<()> = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

    let mut base = std::env::temp_dir();
    base.push(format!(
        "sema-wf-gen-{}-{workflow_name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::env::set_var("SEMA_WORKFLOW_FIXED_TS", "0");
    std::env::remove_var("SEMA_WORKFLOW_RUN_ID");
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &base);
    std::env::set_var("SEMA_WORKFLOW_ARGS_JSON", "{}");
    std::env::remove_var("SEMA_WORKFLOW_RESUME");

    let interp = Interpreter::new();
    reset_runtime_state();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    let _ = interp.eval_str_compiled(src);

    for v in [
        "SEMA_WORKFLOW_FIXED_TS",
        "SEMA_WORKFLOW_RUN_DIR",
        "SEMA_WORKFLOW_ARGS_JSON",
    ] {
        std::env::remove_var(v);
    }

    // Find the generated run dir whose events.jsonl first line is the `run.started` for
    // `workflow_name`; read its events + result.json.
    let mut events = Vec::new();
    let mut result = serde_json::Value::Null;
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let dir = entry.path();
            let evs = std::fs::read_to_string(dir.join("events.jsonl")).unwrap_or_default();
            let parsed: Vec<serde_json::Value> = evs
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str(l).expect("valid event json"))
                .collect();
            let is_target = parsed
                .first()
                .is_some_and(|e| e["event"] == "run.started" && e["workflow"] == workflow_name);
            if is_target {
                result = std::fs::read_to_string(dir.join("result.json"))
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or(serde_json::Value::Null);
                events = parsed;
                break;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    RunOutput {
        events,
        result,
        recorder,
    }
}

/// Convenience: run once into a fresh temp dir (created + cleaned up here).
pub fn run_once(src: &str, fake: FakeProvider, run_id: &str) -> RunOutput {
    let mut dir = std::env::temp_dir();
    dir.push(format!("sema-wf-{}-{}", std::process::id(), run_id));
    let _ = std::fs::remove_dir_all(&dir);
    let out = run_workflow(src, fake, RunOpts::fresh(run_id, &dir));
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// Env vars every workflow test touches — cleared before/after a locked section so no run
/// identity or seam leaks between tests sharing this process-global state.
const WORKFLOW_ENV_VARS: [&str; 6] = [
    "SEMA_WORKFLOW_FIXED_TS",
    "SEMA_WORKFLOW_RUN_ID",
    "SEMA_WORKFLOW_RUN_DIR",
    "SEMA_WORKFLOW_CODE_VERSION",
    "SEMA_WORKFLOW_ARGS_JSON",
    "SEMA_WORKFLOW_RESUME",
];

/// Run `f` while holding the SAME process-wide lock `run_workflow` uses, with all
/// `SEMA_WORKFLOW_*` vars cleared on entry and exit. For tests that drive run-identity /
/// resume gating directly (not through a full `run_workflow`) while sharing the env with
/// the harness so the two never interleave.
pub fn with_workflow_env_lock<T>(f: impl FnOnce() -> T) -> T {
    let _g: MutexGuard<()> = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for v in WORKFLOW_ENV_VARS {
        std::env::remove_var(v);
    }
    let out = f();
    for v in WORKFLOW_ENV_VARS {
        std::env::remove_var(v);
    }
    out
}

/// A unique temp run-dir base for a test (caller removes it when done).
pub fn temp_run_dir(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!("sema-wf-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// All events of a given kind.
pub fn events_of<'a>(events: &'a [serde_json::Value], name: &str) -> Vec<&'a serde_json::Value> {
    events.iter().filter(|e| e["event"] == name).collect()
}
