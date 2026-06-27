//! Deterministic regression tests for the agent-pattern macro cookbook.
//!
//! Each test loads `examples/workflows/cookbook.sema` (embedded inline as a
//! string constant to avoid working-directory sensitivity), defines a
//! `defworkflow` that uses one of the four cookbook macros, and drives it
//! against a scripted `FakeProvider`.  Assertions target real journaled events
//! (`agent.started`, `agent.result`, `run.ended`) plus the final `result.json`
//! envelope so the tests pin observable behaviour, not internal implementation.
//!
//! Shared harness: `workflow_common` (`wc::run_once`, `wc::events_of`).

mod workflow_common;
use workflow_common as wc;

use sema_llm::fake::FakeProvider;

// ── Cookbook source — embedded so the test binary resolves it regardless of cwd ──
//
// We embed the cookbook verbatim via include_str! relative to this file (the
// standard Cargo path anchor).  The macros are then available in any src string
// that prepends COOKBOOK_SRC.
const COOKBOOK_SRC: &str = include_str!("../../../examples/workflows/cookbook.sema");

// ---------------------------------------------------------------------------
// 1. reflexion — short-circuits when the critic replies "OK"
//    Script: attempt reply → critic says "OK" → macro stops after 1 attempt.
//    max-tries = 3, but the "OK" critique on try 1 means only 2 LLM calls:
//      call 1: actor step → "The answer is 42."
//      call 2: critic step → "OK"
//    Result: the first attempt is returned unchanged; only 1 agent.result
//    for the actor (step name "actor") and 1 for the critic (name "critic").
// ---------------------------------------------------------------------------
#[test]
fn reflexion_short_circuits_on_ok_critique() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply("The answer is 42.") // actor attempt
        .reply("OK") // critic — short-circuit
        .build();

    let src = format!(
        r#"
        {COOKBOOK_SRC}

        (defworkflow reflexion-test
          "reflexion macro: stops on OK critique"
          {{:phases ["Run"]}}
          (phase "Run")
          (def r (reflexion "What is 6*7?" 3))
          {{:status :success :r r}})
        "#
    );

    let out = wc::run_once(&src, fake, "wf_reflexion_ok");

    // Run succeeded.
    let ended = wc::events_of(&out.events, "run.ended");
    assert_eq!(ended[0]["status"], "success", "run must succeed");

    // The result carries the first (and only) actor attempt.
    assert_eq!(
        out.result["r"].as_str(),
        Some("The answer is 42."),
        "reflexion must return the first attempt when critique is OK"
    );

    // Exactly 2 agent.started events: one actor, one critic.
    let started = wc::events_of(&out.events, "agent.started");
    assert_eq!(started.len(), 2, "actor + critic = 2 agent.started");
    let names: Vec<&str> = started
        .iter()
        .map(|e| e["agent_name"].as_str().unwrap_or(""))
        .collect();
    assert!(
        names.contains(&"actor"),
        "actor step must be journaled, got: {names:?}"
    );
    assert!(
        names.contains(&"critic"),
        "critic step must be journaled, got: {names:?}"
    );

    // Both steps reported ok.
    let results = wc::events_of(&out.events, "agent.result");
    assert_eq!(
        results.len(),
        2,
        "both actor and critic must journal a result"
    );
    assert!(
        results.iter().all(|r| r["status"] == "ok"),
        "all agent results must be ok"
    );
}

// ---------------------------------------------------------------------------
// 2. reflexion — retries when critique is non-OK, then stops at max-tries
//    max-tries = 2 → attempt 1, critique 1 (non-OK), attempt 2 returned
//    even though attempt-2 also got a bad critic (but we've hit the cap).
//    Script: 4 calls:
//      call 1: actor attempt 1 → "First draft."
//      call 2: critic         → "Needs work: be concise."
//      call 3: actor attempt 2 (with critique) → "Revised draft."
//      (no call 4: max-tries reached, no more critic)
// ---------------------------------------------------------------------------
#[test]
fn reflexion_retries_on_non_ok_critique_then_stops_at_max() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply("First draft.")
        .reply("Needs work: be concise.")
        .reply("Revised draft.")
        .build();

    let src = format!(
        r#"
        {COOKBOOK_SRC}

        (defworkflow reflexion-retry-test
          "reflexion macro: retries on non-OK critique"
          {{:phases ["Run"]}}
          (phase "Run")
          (def r (reflexion "Write a haiku." 2))
          {{:status :success :r r}})
        "#
    );

    let out = wc::run_once(&src, fake, "wf_reflexion_retry");

    let ended = wc::events_of(&out.events, "run.ended");
    assert_eq!(ended[0]["status"], "success");

    // The last attempt (try 2 = max) is returned.
    assert_eq!(
        out.result["r"].as_str(),
        Some("Revised draft."),
        "reflexion must return the final attempt when max-tries is hit"
    );

    // 3 agent.started: actor-1, critic-1, actor-2.
    let started = wc::events_of(&out.events, "agent.started");
    assert_eq!(
        started.len(),
        3,
        "actor-1 + critic-1 + actor-2 = 3 agent.started"
    );
}

// ---------------------------------------------------------------------------
// 3. react — stops after round 1 because the reply has no "next:" sentinel
//    The step has :tools so it routes through the real tool loop.
//    Script:
//      call 1: tool_call "get-weather" → Oslo
//      call 2: final reply "It is 22°C in Oslo." (no "next:")
//    After round 1 the macro checks: NOT contains "next:" → return answer.
//    Assertions: 1 agent.tool_call, 1 agent.result, final answer returned.
// ---------------------------------------------------------------------------

const WEATHER_TOOL: &str = r#"
    (deftool get-weather
      "Get current weather for a city"
      {:city {:type :string :description "City name"}}
      (lambda (city) (str "{\"city\":\"" city "\",\"temp\":22}")))
"#;

#[test]
fn react_stops_after_one_round_no_next_sentinel() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call(
            "call_1",
            "get-weather",
            serde_json::json!({ "city": "Oslo" }),
        )
        .reply("It is 22°C in Oslo.")
        .build();

    let src = format!(
        r#"
        {WEATHER_TOOL}
        {COOKBOOK_SRC}

        (defworkflow react-test
          "react macro: stops when reply has no next: sentinel"
          {{:phases ["Run"]}}
          (phase "Run")
          (def r (react "What is the weather in Oslo?" [get-weather] 4))
          {{:status :success :r r}})
        "#
    );

    let out = wc::run_once(&src, fake, "wf_react_no_next");

    // Run succeeded.
    assert_eq!(
        wc::events_of(&out.events, "run.ended")[0]["status"],
        "success"
    );

    // The final answer is returned.
    assert_eq!(
        out.result["r"].as_str(),
        Some("It is 22°C in Oslo."),
        "react must return the final answer"
    );

    // Exactly ONE agent.tool_call — the tool loop ran once.
    let tool_calls = wc::events_of(&out.events, "agent.tool_call");
    assert_eq!(tool_calls.len(), 1, "one tool call must be journaled");
    assert_eq!(tool_calls[0]["tool_name"], "get-weather");

    // One agent.result (the react step itself).
    let results = wc::events_of(&out.events, "agent.result");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["status"], "ok");
}

// ---------------------------------------------------------------------------
// 4. tree-of-thought — picks the longest candidate (by string-length scorer)
//    n=2 candidates, scorer = string-length.
//    Script:
//      call 1: candidate 1 → "Short."
//      call 2: candidate 2 → "A much longer candidate answer."
//    foldl argmax with string-length → "A much longer candidate answer."
//    Assertions: 2 agent.started events, result is the longer string.
// ---------------------------------------------------------------------------
#[test]
fn tree_of_thought_picks_best_by_scorer() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply("Short.")
        .reply("A much longer candidate answer.")
        .build();

    let src = format!(
        r#"
        {COOKBOOK_SRC}

        (defworkflow tot-test
          "tree-of-thought: picks highest-scoring candidate"
          {{:phases ["Run"]}}
          (phase "Run")
          (def r (tree-of-thought "Name this library." 2 (fn (c) (string-length c))))
          {{:status :success :r r}})
        "#
    );

    let out = wc::run_once(&src, fake, "wf_tot_picks_best");

    assert_eq!(
        wc::events_of(&out.events, "run.ended")[0]["status"],
        "success"
    );

    // The longer candidate wins under a string-length scorer.
    assert_eq!(
        out.result["r"].as_str(),
        Some("A much longer candidate answer."),
        "tree-of-thought must pick the highest-scoring candidate"
    );

    // 2 agent.started events — both candidates ran.
    let started = wc::events_of(&out.events, "agent.started");
    assert_eq!(
        started.len(),
        2,
        "both thought candidates must be journaled"
    );

    // Both steps produced results.
    let results = wc::events_of(&out.events, "agent.result");
    assert_eq!(results.len(), 2);
    assert!(
        results.iter().all(|r| r["status"] == "ok"),
        "all candidate results must be ok"
    );
}

// ---------------------------------------------------------------------------
// 5. debate — 1 round, judge decides
//    rounds=1: one pair of persona steps (Pro + Con) then judge.
//    Script:
//      call 1: Pro  → "Dynamic typing is liberating."
//      call 2: Con  → "Types catch bugs early."
//      call 3: judge → "Pro wins: flexibility matters more."
//    Assertions: 3 agent.started (Pro, Con, judge), result = judge verdict.
// ---------------------------------------------------------------------------
#[test]
fn debate_runs_one_round_and_returns_judge_verdict() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply("Dynamic typing is liberating.")
        .reply("Types catch bugs early.")
        .reply("Pro wins: flexibility matters more.")
        .build();

    let src = format!(
        r#"
        {COOKBOOK_SRC}

        (defworkflow debate-test
          "debate macro: 1 round, judge decides"
          {{:phases ["Run"]}}
          (phase "Run")
          (def r (debate "Is dynamic typing good?" "Pro" "Con" 1))
          {{:status :success :r r}})
        "#
    );

    let out = wc::run_once(&src, fake, "wf_debate_judge");

    assert_eq!(
        wc::events_of(&out.events, "run.ended")[0]["status"],
        "success"
    );

    // The judge's verdict is the return value.
    assert_eq!(
        out.result["r"].as_str(),
        Some("Pro wins: flexibility matters more."),
        "debate must return the judge's verdict"
    );

    // 3 agent.started: Pro, Con, judge.
    let started = wc::events_of(&out.events, "agent.started");
    assert_eq!(started.len(), 3, "Pro + Con + judge = 3 agent.started");
    let names: Vec<&str> = started
        .iter()
        .map(|e| e["agent_name"].as_str().unwrap_or(""))
        .collect();
    assert!(names.contains(&"Pro"), "Pro persona must be journaled");
    assert!(names.contains(&"Con"), "Con persona must be journaled");
    assert!(names.contains(&"judge"), "judge step must be journaled");

    // All 3 steps reported ok.
    let results = wc::events_of(&out.events, "agent.result");
    assert_eq!(results.len(), 3);
    assert!(
        results.iter().all(|r| r["status"] == "ok"),
        "all debate results must be ok"
    );
}

// ---------------------------------------------------------------------------
// 6. debate — 2 rounds to confirm the loop works across multiple rounds
//    rounds=2: Pro-1, Con-1, Pro-2, Con-2, then judge.
//    Script: 5 replies in order.
// ---------------------------------------------------------------------------
#[test]
fn debate_runs_two_rounds_then_judge() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply("Pro round 1.")
        .reply("Con round 1.")
        .reply("Pro round 2.")
        .reply("Con round 2.")
        .reply("Judge: Con wins.")
        .build();

    let src = format!(
        r#"
        {COOKBOOK_SRC}

        (defworkflow debate-2r-test
          "debate macro: 2 rounds then judge"
          {{:phases ["Run"]}}
          (phase "Run")
          (def r (debate "Should tabs beat spaces?" "Tabs" "Spaces" 2))
          {{:status :success :r r}})
        "#
    );

    let out = wc::run_once(&src, fake, "wf_debate_2rounds");

    assert_eq!(
        wc::events_of(&out.events, "run.ended")[0]["status"],
        "success"
    );

    // Judge verdict returned.
    assert_eq!(
        out.result["r"].as_str(),
        Some("Judge: Con wins."),
        "debate must return judge verdict after 2 rounds"
    );

    // 5 agent.started: Tabs, Spaces, Tabs, Spaces, judge.
    let started = wc::events_of(&out.events, "agent.started");
    assert_eq!(
        started.len(),
        5,
        "2*2 persona steps + 1 judge = 5 agent.started"
    );
}
