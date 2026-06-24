//! Deterministic budget-enforcement tests (slice S1).
//!
//! Enforcement is a sticky `over_budget` latch on `WorkflowCtx`, set by `charge` once a
//! `:budget {:tokens N}` cap is exceeded and checked at agent ENTRY — NOT `Err`
//! propagation, which the `__fanout-tagged` engine swallows into `nil`. Driven in-process
//! against a scripted `FakeProvider` (no network/keys), so a `:tokens` cap trips
//! deterministically (a `:usd` cap would couple the test to the pricing table). The
//! shared harness lives in `workflow_common`.

mod workflow_common;
use workflow_common as wc;

use sema_llm::fake::FakeProvider;
use sema_llm::types::LlmError;

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

    let out = wc::run_once(src, fake, "wf_budget_seq");

    // The run is forced to :failed with the budget reason, regardless of the body's
    // own (successful) last value.
    let ended = wc::events_of(&out.events, "run.ended");
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0]["status"], "failed");
    assert_eq!(ended[0]["reason"], "budget exceeded");
    assert_eq!(out.result["status"], "failed");
    assert_eq!(out.result["reason"], "budget exceeded");

    // Exactly ONE agent launched — the second was refused at entry (no events).
    let started = wc::events_of(&out.events, "agent.started");
    assert_eq!(
        started.len(),
        1,
        "second agent must not launch after the cap"
    );
    assert_eq!(started[0]["agent_name"], "one");

    // The tipping leaf's Budget event carries the populated token cap.
    let budget = wc::events_of(&out.events, "budget");
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

    let out = wc::run_once(src, fake, "wf_budget_failed");

    let budget = wc::events_of(&out.events, "budget");
    assert_eq!(
        budget.len(),
        1,
        "the failed leaf must not emit a budget event"
    );
    assert_eq!(budget[0]["agent_id"], "one_1");

    // The failed leaf is still recorded as a failed agent result.
    let results = wc::events_of(&out.events, "agent.result");
    let two = results.iter().find(|e| e["agent_id"] == "two_1").unwrap();
    assert_eq!(two["status"], "failed");
}

#[test]
fn usd_budget_enforced_end_to_end_via_pinned_pricing() {
    // The :usd path (cost_usd → charge → latch → forced :failed) was only covered by
    // direct charge() unit tests. Pin the fake model's price (via the Sema builtin, so
    // it survives the harness's reset_runtime_state) so one reply blows a tiny :usd cap.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply_with_usage("first", 10, 5)
        .reply_with_usage("second", 10, 5)
        .build();

    let src = r#"
        (llm/set-pricing "fake-model" 1000.0 1000.0)   ; $/1M tokens
        (defworkflow budget-usd
          "tiny usd cap"
          {:phases ["A"] :budget {:usd 0.001}}
          (phase "A")
          (def x (agent "first" {:name "one"}))
          (def y (agent "second" {:name "two"}))
          {:status :success})
    "#;
    let out = wc::run_once(src, fake, "wf_budget_usd");

    // 15 tokens × $1000/1M = $0.015 ≫ $0.001 cap → run fails, second agent refused,
    // and the tipping budget event carries a real cost.
    assert_eq!(out.result["status"], "failed");
    assert_eq!(out.result["reason"], "budget exceeded");
    let started = wc::events_of(&out.events, "agent.started");
    assert_eq!(started.len(), 1, "second agent refused after the usd cap");
    let budget = wc::events_of(&out.events, "budget");
    assert!(budget[0]["cost_usd"].as_f64().unwrap() > 0.0);
}

#[test]
fn budget_latch_fails_run_and_refuses_further_leaves_under_fanout() {
    // A pipeline fan-out over 20 items with a tiny budget. The latch can't stop siblings
    // already in flight (LAST_USAGE accounting is best-effort under fan-out), but once it
    // trips, queued leaves are refused at entry — so strictly fewer than 20 agents run.
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

    let out = wc::run_once(src, fake, "wf_budget_fanout");

    let ended = wc::events_of(&out.events, "run.ended");
    assert_eq!(ended[0]["status"], "failed");
    assert_eq!(ended[0]["reason"], "budget exceeded");
    assert_eq!(out.result["status"], "failed");

    let started = wc::events_of(&out.events, "agent.started");
    assert!(
        !started.is_empty() && started.len() < 20,
        "latch must refuse further leaves under fan-out: ran {} of 20",
        started.len()
    );
}
