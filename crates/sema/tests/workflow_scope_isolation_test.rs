//! Per-task ISOLATION of the live workflow run scope under cooperative interleaving.
//!
//! A workflow run's [`WorkflowCtx`] is a scope on the OWNING TASK (a traced
//! `WorkflowTaskState` extension on the task context), not a thread-local. Two tasks
//! interleaved on one thread — the parent, an unrelated sibling root, a spawned child —
//! therefore each resolve `workflow/checkpoint` / `workflow/tool-call` against THEIR OWN
//! task, never a run that merely happens to be parked on the thread. `cur_agent` is
//! TASK-PRIVATE so concurrent steps never cross-attribute their tool calls, and a spawned
//! child inherits its spawner's run + step attribution. Companion to
//! `task_scope_isolation_test.rs` (OTel span-stack isolation) and
//! `workflow_checkpoint_async_test.rs` (cooperative checkpoint writes).

#![cfg(not(target_arch = "wasm32"))]

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Mutex;

use sema_core::Value;
use sema_eval::Interpreter;
use sema_stdlib::workflow_mcp::{
    clear_workflow_mcp_resolver, set_workflow_mcp_resolver, McpDecl, ServerResolution,
    WorkflowMcpResolver,
};

// The `SEMA_WORKFLOW_*` env + the thread-local MCP resolver registry are process-global,
// so these tests serialize within this binary (separate binaries are separate processes).
static SERIAL: Mutex<()> = Mutex::new(());

struct WfRun {
    value: Result<Value, String>,
    events: Vec<serde_json::Value>,
}

/// Evaluate `src` as a workflow program under the fixed-ts seam into a private temp dir,
/// returning the eval result value AND the events file written by `run_id`.
fn run_src(src: &str, run_id: &str) -> WfRun {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("sema-wfiso-{}-{run_id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("SEMA_WORKFLOW_FIXED_TS", "0");
    std::env::set_var("SEMA_WORKFLOW_RUN_ID", run_id);
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &dir);

    let interp = Interpreter::new();
    let value = interp.eval_str_compiled(src).map_err(|e| e.to_string());

    for v in [
        "SEMA_WORKFLOW_FIXED_TS",
        "SEMA_WORKFLOW_RUN_ID",
        "SEMA_WORKFLOW_RUN_DIR",
    ] {
        std::env::remove_var(v);
    }

    let events = read_events(&dir.join(run_id).join("events.jsonl"));
    let _ = std::fs::remove_dir_all(&dir);
    WfRun { value, events }
}

fn read_events(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid event json"))
        .collect()
}

fn events_of<'a>(events: &'a [serde_json::Value], name: &str) -> Vec<&'a serde_json::Value> {
    events.iter().filter(|e| e["event"] == name).collect()
}

// ── parent / sibling checkpoints resolve to their OWN task, not a parked run ──

#[test]
fn parent_checkpoint_while_child_parked_in_workflow_errors_outside_run() {
    // A child task runs a workflow and parks INSIDE its body. Meanwhile the parent (root)
    // task — which is NOT in any workflow — checkpoints. With the thread-local scope it
    // resolved to the child's run and wrote into its journal; task-local scope makes it
    // correctly report "outside a workflow/run".
    let out = run_src(
        r#"
        (def wf
          (async/spawn
            (fn ()
              (workflow/run "inner" "doc" {}
                (fn () (async/sleep 300) :done)))))
        (async/sleep 60)
        (def root-check
          (try (workflow/checkpoint :leak (fn () 1)) (catch error :outside)))
        (async/await wf)
        root-check
        "#,
        "wf_parent_iso",
    );
    assert_eq!(
        out.value.expect("program evaluated"),
        Value::keyword("outside"),
        "the parent, outside any workflow, must not resolve to the child's parked run"
    );
    // And nothing leaked a `:leak` checkpoint into the child's journal.
    assert!(
        events_of(&out.events, "checkpoint")
            .iter()
            .all(|e| e["key"] != "leak"),
        "the parent's checkpoint must never land in the child run's journal"
    );
}

#[test]
fn unrelated_root_and_second_interpreter_see_no_workflow() {
    // A sibling spawned task (unrelated to the run) and the root both see "outside" while
    // another task is parked inside a workflow.
    let out = run_src(
        r#"
        (def wf
          (async/spawn
            (fn ()
              (workflow/run "inner" "doc" {}
                (fn () (async/sleep 300) :done)))))
        (def sibling
          (async/spawn
            (fn ()
              (async/sleep 30)
              (try (workflow/checkpoint :s (fn () 1)) (catch error :outside)))))
        (async/sleep 60)
        (def root-check
          (try (workflow/checkpoint :r (fn () 1)) (catch error :outside)))
        (def sib (async/await sibling))
        (async/await wf)
        (list root-check sib)
        "#,
        "wf_unrelated_iso",
    );
    assert_eq!(
        out.value.expect("program evaluated"),
        Value::list(vec![Value::keyword("outside"), Value::keyword("outside")]),
        "an unrelated root AND an unrelated sibling task must both see no workflow"
    );

    // A brand-new interpreter (fresh root task) also sees no workflow — no thread leakage.
    let fresh = Interpreter::new();
    let v = fresh
        .eval_str_compiled(r#"(try (workflow/checkpoint :x (fn () 1)) (catch error :outside))"#)
        .expect("fresh interpreter evaluated");
    assert_eq!(v, Value::keyword("outside"));
}

// ── concurrent steps keep distinct agent attribution ──────────────────────

#[test]
fn concurrent_steps_keep_separate_agent_ids() {
    // Two parallel `workflow/step` leaves each set their active agent, park mid-step, then
    // tool-call. A single shared `cur_agent` slot collapsed both tool calls onto the
    // last-set agent (and cleared it before the second leaf resumed, dropping its event);
    // the per-task slot keeps each leaf attributed to its own step.
    let out = run_src(
        r#"
        (workflow/run "concurrent" "doc" {}
          (fn ()
            (def a
              (async/spawn
                (fn () (workflow/step {:name "scout"}
                          (fn () (async/sleep 60) (workflow/tool-call :probe) 1)))))
            (def b
              (async/spawn
                (fn () (workflow/step {:name "scout"}
                          (fn () (async/sleep 60) (workflow/tool-call :probe) 2)))))
            (async/await a)
            (async/await b)
            {:status :success}))
        "#,
        "wf_concurrent_iso",
    );
    out.value.expect("program evaluated");

    let tool_calls = events_of(&out.events, "agent.tool_call");
    assert_eq!(
        tool_calls.len(),
        2,
        "each concurrent step must emit its own tool-call event, not lose one to a shared slot"
    );
    let agent_ids: BTreeSet<&str> = tool_calls
        .iter()
        .filter_map(|e| e["agent_id"].as_str())
        .collect();
    assert_eq!(
        agent_ids.len(),
        2,
        "the two tool calls must attribute to two distinct agent ids ({agent_ids:?})"
    );
    // Every tool-call agent id is a real step (has a matching agent.started).
    let started: BTreeSet<&str> = events_of(&out.events, "agent.started")
        .iter()
        .filter_map(|e| e["agent_id"].as_str())
        .collect();
    assert!(
        agent_ids.is_subset(&started),
        "tool calls must attribute to launched steps ({agent_ids:?} ⊄ {started:?})"
    );
}

// ── a spawned child inherits the run AND the active step attribution ───────

#[test]
fn spawned_child_inherits_workflow_and_step_attribution() {
    // A step spawns a child; the child journals into the SAME run (inherited scope) and
    // its tool call attributes to the SPAWNER's step (inherited cur_agent).
    let out = run_src(
        r#"
        (workflow/run "inherit" "doc" {}
          (fn ()
            (workflow/step {:name "lead"}
              (fn ()
                (def kid
                  (async/spawn
                    (fn () (workflow/tool-call :child-probe) :kid)))
                (async/await kid)
                :lead-done))
            {:status :success}))
        "#,
        "wf_inherit_iso",
    );
    out.value.expect("program evaluated");

    let tool_calls = events_of(&out.events, "agent.tool_call");
    assert_eq!(tool_calls.len(), 1, "the child's tool call must reach the run journal");
    assert_eq!(tool_calls[0]["tool_name"], "child-probe");
    let lead = events_of(&out.events, "agent.started")
        .iter()
        .find_map(|e| e["agent_id"].as_str().map(str::to_owned))
        .expect("the lead step started");
    assert_eq!(
        tool_calls[0]["agent_id"], lead,
        "the child's tool call inherits the spawner step's attribution"
    );
}

// ── nested runs restore the exact outer scope ──────────────────────────────

#[test]
fn nested_runs_restore_exact_outer_scope_across_interleaved_teardown() {
    // A nested `workflow/run` installs and tears down its own scope; when it returns, the
    // OUTER run's scope is exactly restored, so the outer checkpoint reads back its own
    // value (not the inner run's).
    let out = run_src(
        r#"
        (workflow/run "outer" "doc" {}
          (fn ()
            (workflow/checkpoint :marker (fn () :outer-value))
            (def inner
              (workflow/run "inner" "doc" {}
                (fn () (workflow/checkpoint :marker (fn () :inner-value)))))
            {:status :success
             :outer-readback (workflow/checkpoint :marker)
             :inner-status (:status inner)}))
        "#,
        "wf_nested_iso",
    );
    let value = out.value.expect("program evaluated");
    let rendered = sema_core::pretty_print(&value, 100);
    assert!(
        rendered.contains(":outer-readback :outer-value"),
        "outer scope must be restored after the nested run returns: {rendered}"
    );
    assert!(
        rendered.contains(":inner-status :success"),
        "the nested run must complete on its own scope: {rendered}"
    );
}

// ── cancellation tears the scope down and closes MCP exactly once ──────────

struct FakeResolver {
    close_calls: Rc<Cell<u32>>,
}

impl WorkflowMcpResolver for FakeResolver {
    fn resolve(&self, decls: &[McpDecl], _workflow: &str, _run_id: &str) -> Vec<ServerResolution> {
        decls
            .iter()
            .map(|d| ServerResolution::Connected {
                alias: d.alias.clone(),
                handle: Value::string(&format!("handle-{}", d.alias)),
                auth: None,
            })
            .collect()
    }

    fn close(&self, _handles: &[Value]) {
        self.close_calls.set(self.close_calls.get() + 1);
    }
}

#[test]
fn cancelled_run_removes_scope_token_and_closes_mcp_handles_once() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let close_calls = Rc::new(Cell::new(0u32));
    set_workflow_mcp_resolver(Rc::new(FakeResolver {
        close_calls: Rc::clone(&close_calls),
    }));

    let dir = std::env::temp_dir().join(format!("sema-wfiso-{}-cancel", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("SEMA_WORKFLOW_FIXED_TS", "0");
    std::env::set_var("SEMA_WORKFLOW_RUN_ID", "wf_cancel_iso");
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &dir);

    let interp = Interpreter::new();
    // Spawn a run that resolves its MCP server (connected) then parks forever in its body;
    // cancel it mid-body. The teardown must close the handle exactly once and remove the
    // scope, leaving a fresh checkpoint call "outside a workflow/run".
    let _ = interp.eval_str_compiled(
        r#"
        (def r
          (async/spawn
            (fn ()
              (workflow/run "cancelme" "doc"
                {:mcp (quote {asana {:url "https://example.test/mcp"}})}
                (fn () (async/sleep 100000) :never)))))
        (async/sleep 60)
        (async/cancel r)
        (try (async/await r) (catch error nil))
        (try (workflow/checkpoint :after (fn () 1)) (catch error :outside))
        "#,
    );

    for v in [
        "SEMA_WORKFLOW_FIXED_TS",
        "SEMA_WORKFLOW_RUN_ID",
        "SEMA_WORKFLOW_RUN_DIR",
    ] {
        std::env::remove_var(v);
    }
    clear_workflow_mcp_resolver();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        close_calls.get(),
        1,
        "a cancelled run must close its MCP handle exactly once"
    );

    // The scope was removed: a fresh top-level checkpoint (fresh interpreter) sees nothing.
    let fresh = Interpreter::new();
    let v = fresh
        .eval_str_compiled(r#"(try (workflow/checkpoint :x (fn () 1)) (catch error :outside))"#)
        .expect("fresh interpreter evaluated");
    assert_eq!(v, Value::keyword("outside"));
}
