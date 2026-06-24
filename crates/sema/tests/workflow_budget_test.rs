//! Deterministic budget-enforcement tests (track #2, slice S1).
//!
//! Enforcement is a sticky `over_budget` latch on `WorkflowCtx`, set by `charge` once
//! a `:budget {:tokens N}` cap is exceeded and checked at agent ENTRY — NOT `Err`
//! propagation, which the `__fanout-tagged` engine would swallow into `nil`. These
//! tests drive a workflow in-process against a scripted `FakeProvider` (no network,
//! no keys), so token usage is deterministic and a `:tokens` cap trips predictably
//! (a `:usd` cap would couple the test to the pricing table — see the design doc).
//!
//! Env isolation: this is its own test binary (its own process), so the
//! `SEMA_WORKFLOW_*` env vars it sets do not leak into other binaries. A process-wide
//! `SERIAL` mutex serializes the tests within THIS binary (they share the env + the
//! thread-local provider registry).

use std::sync::{Mutex, MutexGuard};

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::FakeProvider;
use sema_llm::types::LlmError;

static SERIAL: Mutex<()> = Mutex::new(());

/// Run `src` as a workflow under the fixed-ts/run-id seam into a fresh temp run dir,
/// against `fake` as the default provider. Returns the parsed events of the run.
fn run_workflow(
    src: &str,
    fake: FakeProvider,
    run_id: &str,
) -> (Vec<serde_json::Value>, serde_json::Value) {
    let _guard: MutexGuard<()> = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

    let mut dir = std::env::temp_dir();
    dir.push(format!("sema-wf-budget-{}-{}", std::process::id(), run_id));
    let _ = std::fs::remove_dir_all(&dir);

    std::env::set_var("SEMA_WORKFLOW_FIXED_TS", "0");
    std::env::set_var("SEMA_WORKFLOW_RUN_ID", run_id);
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &dir);

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));
    let _ = interp.eval_str_compiled(src);

    std::env::remove_var("SEMA_WORKFLOW_FIXED_TS");
    std::env::remove_var("SEMA_WORKFLOW_RUN_ID");
    std::env::remove_var("SEMA_WORKFLOW_RUN_DIR");

    let run = dir.join(run_id);
    let events = std::fs::read_to_string(run.join("events.jsonl")).expect("events.jsonl written");
    let parsed: Vec<serde_json::Value> = events
        .lines()
        .map(|l| serde_json::from_str(l).expect("valid event json"))
        .collect();
    let result: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(run.join("result.json")).expect("result.json"),
    )
    .expect("valid result json");
    let _ = std::fs::remove_dir_all(&dir);
    (parsed, result)
}

fn events_of<'a>(events: &'a [serde_json::Value], name: &str) -> Vec<&'a serde_json::Value> {
    events.iter().filter(|e| e["event"] == name).collect()
}

#[test]
fn budget_stops_sequential_run_after_the_tipping_leaf() {
    // Agent "one" burns 15 tokens against a 5-token cap → the latch trips after its
    // Budget event. Agent "two" must then be refused at entry: no agent.started for it.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply_with_usage("first answer", 10, 5)
        .reply_with_usage("second answer", 10, 5)
        .build();

    let src = r#"
        (defworkflow budget-demo
          "two sequential agents, tiny token budget"
          {:phases ["A" "B"] :budget {:tokens 5}}
          (phase "A")
          (def x (agent "first" {:name "one"}))
          (phase "B")
          (def y (agent "second" {:name "two"}))
          {:status :success :x x :y y})
    "#;

    let (events, result) = run_workflow(src, fake, "wf_budget_seq");

    // The run is forced to :failed with the budget reason, regardless of the body's
    // own (successful) last value.
    let ended = events_of(&events, "run.ended");
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0]["status"], "failed");
    assert_eq!(ended[0]["reason"], "budget exceeded");
    assert_eq!(result["status"], "failed");
    assert_eq!(result["reason"], "budget exceeded");

    // Exactly ONE agent launched — the second was refused at entry (no events).
    let started = events_of(&events, "agent.started");
    assert_eq!(
        started.len(),
        1,
        "second agent must not launch after the cap"
    );
    assert_eq!(started[0]["agent_name"], "one");

    // The tipping leaf's Budget event carries the populated token cap.
    let budget = events_of(&events, "budget");
    assert_eq!(budget.len(), 1);
    assert_eq!(budget[0]["budget_limit"], 5);
}

#[test]
fn failed_leaf_does_not_emit_a_phantom_budget_from_stale_usage() {
    // Regression (verification bug #1): a leaf whose LLM call FAILS made no completion,
    // so it must report ZERO usage — NOT re-read the previous leaf's sticky LAST_USAGE.
    // Agent "one" succeeds (15 tokens); agent "two" errors. There must be exactly ONE
    // budget event (for "one"); "two" must not emit a phantom budget carrying one's
    // tokens (which would also double-charge the cap).
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply_with_usage("first answer", 10, 5)
        .error(LlmError::Api {
            status: 500,
            message: "boom".into(),
        })
        .build();

    let src = r#"
        (defworkflow budget-failed-leaf
          "second agent's call fails"
          {:phases ["A"] :budget {:tokens 1000}}
          (phase "A")
          (def x (agent "first" {:name "one"}))
          (def y (agent "second" {:name "two"}))
          {:status :success})
    "#;

    let (events, _result) = run_workflow(src, fake, "wf_budget_failed");

    let budget = events_of(&events, "budget");
    assert_eq!(
        budget.len(),
        1,
        "the failed leaf must not emit a budget event"
    );
    assert_eq!(budget[0]["agent_id"], "one_1");

    // The failed leaf is still recorded as a failed agent result.
    let results = events_of(&events, "agent.result");
    let two = results.iter().find(|e| e["agent_id"] == "two_1").unwrap();
    assert_eq!(two["status"], "failed");
}

#[test]
fn budget_latch_fails_run_and_refuses_further_leaves_under_fanout() {
    // A pipeline fan-out over 20 items with a tiny budget. The latch can't stop
    // siblings already in flight (LAST_USAGE accounting is best-effort under fan-out),
    // but once it trips, queued leaves are refused at entry — so strictly fewer than
    // 20 agents run, and the run is forced :failed.
    let mut b = FakeProvider::builder("fake").model("fake-model");
    for _ in 0..20 {
        b = b.reply_with_usage("x", 10, 5);
    }
    let fake = b.build();

    let src = r#"
        (defworkflow budget-fanout
          "fan-out with a tiny token budget"
          {:phases ["Work"] :budget {:tokens 5}}
          (phase "Work")
          (def rs (pipeline (range 20) (fn (i) (agent (str "item " i) {:name "w"}))))
          {:status :success :n (count rs)})
    "#;

    let (events, result) = run_workflow(src, fake, "wf_budget_fanout");

    let ended = events_of(&events, "run.ended");
    assert_eq!(ended[0]["status"], "failed");
    assert_eq!(ended[0]["reason"], "budget exceeded");
    assert_eq!(result["status"], "failed");

    let started = events_of(&events, "agent.started");
    assert!(
        !started.is_empty() && started.len() < 20,
        "latch must refuse further leaves under fan-out: ran {} of 20",
        started.len()
    );
}
