//! Acceptance oracle for **non-blocking multi-round `agent/run`** (issue #61 §3a /
//! canonical cooperative-scheduling plan M1).
//!
//! A single `llm/complete` already offloads + yields `AwaitIo`, so concurrent
//! completions overlap. But `agent/run` drives `run_tool_loop` — a Rust `for` over
//! rounds that calls the *synchronous* `do_complete`, so a multi-round agent
//! conversation freezes every sibling task for its WHOLE duration. These tests pin
//! the fix: each provider round must offload + yield so siblings (other agents, a
//! render/ticker task) run during every inter-round wait.
//!
//! Deterministic + keyless: a `FakeProvider` with `tool_loop` (a request-keyed
//! multi-round script that stays reproducible under ANY interleaving — see
//! `FakeProvider::tool_loop`) plus an injected `chat_delay` per round. Overlap is
//! proven two ways: peak offloaded futures in flight ≥ 2, and a sibling ticker
//! that advances *during* the agent's rounds.
//!
//! Own binary — the `IO_INFLIGHT` instrumentation atomics and the provider registry
//! are process-global, so these `#[serial]` tests must not share a process with
//! unrelated inflight capture.

//! CONCURRENCY ORACLE: `io_peak_inflight() >= 2`, never a wall-clock ceiling.
//!
//! The `concurrent_agents_overlap_*` tests prove overlap with that lower bound
//! alone: serial execution can never have two offloaded provider rounds in
//! flight at once, and a lower bound holds no matter how loaded the machine
//! is. They deliberately assert no `wall_ms < N` upper bound — an elapsed-time
//! ceiling is load-sensitive by construction and fails intermittently under a
//! full-workspace `cargo nextest run` while passing in isolation (see
//! docs/bugs/2026-07-28-sibling-interleaving-tests-are-load-sensitive.md).

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{
    agent_runs_len, io_peak_inflight, register_test_provider, reset_io_inflight,
    reset_runtime_state,
};
use sema_llm::fake::FakeProvider;
use serial_test::serial;

/// N agents, each a 2-tool-round conversation (3 provider calls) with a 120 ms
/// delay per round, spawned concurrently. Blocking would mean each agent hogs
/// the VM thread through all its rounds, so they run serially and peak
/// in-flight stays at 1. Non-blocking: every round offloads + yields, so the
/// agents overlap and peak in-flight reaches ≥ 2 — the load-safe concurrency
/// oracle (a wall-clock ceiling would be load-sensitive; see the module note).
#[test]
#[serial]
fn concurrent_agents_overlap_and_peak_inflight() {
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(120)
        // 2 tool rounds then a final reply, keyed per-request so all three agents
        // stay deterministic no matter how their rounds interleave.
        .tool_loop(2, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // 3 agents × (2 tool rounds + 1 final) × 120 ms per round.
    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 6})
        (async/all
          (map (fn (i) (async/spawn (fn () (agent/run bot "go"))))
               (list 1 2 3)))
    "#;
    interp
        .eval_str_compiled(program)
        .expect("3 concurrent agents evaluated");

    assert!(
        io_peak_inflight() >= 2,
        "expected peak offloaded futures in flight >= 2 (agents overlapping across \
         rounds), got {} — agent/run still blocks per round",
        io_peak_inflight()
    );
}

/// A sibling "ticker" task (increments a channel on a virtual-time cadence) must
/// make progress *while* one agent runs its rounds. The agent snapshots the tick
/// count on each tool-call via `:on-tool-call`. Blocking today: the agent runs all
/// rounds atomically, the ticker never advances during it, every snapshot is 0.
/// Non-blocking: the ticker interleaves during each inter-round park, so a later
/// round observes a strictly higher count → the max snapshot is > 0.
#[test]
#[serial]
fn sibling_ticker_advances_during_agent_rounds() {
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(60)
        .tool_loop(3, "noop", serde_json::json!({ "x": "y" }), "final")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (deftool noop "noop" {:x {:type :string}} (fn (x) "ok"))
        (defagent bot {:model "fake-model" :tools [noop] :max-turns 8})
        (define ticks (channel/new 500))
        (define snaps (channel/new 64))
        (define (ticker)
          (dotimes (i 60)
            (async/sleep 5)
            (channel/send ticks 1)))
        (define (run-agent)
          (agent/run bot "go"
            {:on-tool-call
             (fn (ev)
               (if (= (:event ev) "start")
                   (channel/send snaps (channel/count ticks))))}))
        (async/all (list (async/spawn (fn () (ticker)))
                         (async/spawn (fn () (run-agent)))))
        (define (drain-max ch acc)
          (let ((v (channel/try-recv ch)))
            (if (nil? v) acc (drain-max ch (max acc v)))))
        (drain-max snaps 0)
    "#;
    let max_snap = interp
        .eval_str_compiled(program)
        .expect("ticker + agent evaluated");
    assert!(
        max_snap.as_int().unwrap_or(0) > 0,
        "expected the ticker to advance DURING the agent's rounds (max snapshot > 0), \
         got {max_snap} — agent/run froze the sibling task"
    );
}

/// Explicitly cancelling a running agent before the full conversation completes
/// must cut the round loop short: no NEW provider round starts after the cutoff.
/// Blocking today: `agent/run` is one synchronous native call the scheduler cannot
/// interrupt, so the cancellation task can't run until the agent has
/// already run every round — all `full` provider calls happen. Non-blocking: the
/// agent parks on `AwaitIo` between rounds, explicit cancellation stops it there, and the
/// remaining rounds never dispatch (best-effort for the in-flight round).
#[test]
#[serial]
fn cancelling_agent_run_cuts_the_loop_short() {
    reset_io_inflight();

    // 8 tool rounds (9 calls) at 100 ms each ⇒ ~900 ms full; cancel at 250 ms.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(100)
        .tool_loop(8, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));

    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 12})
        (let ((p (async/spawn (fn () (agent/run bot "go")))))
          (async/spawn (fn () (async/sleep 250) (async/cancel p)))
          ;; Observe the explicit cancellation; the assertion is on dispatched rounds.
          (try (async/await p) (catch e nil)))
    "#;
    let _ = interp.eval_str_compiled(program);

    let calls = recorder.call_count();
    assert!(
        calls > 0 && calls < 9,
        "expected the cancelled agent to stop short of all 9 provider rounds, but it \
         made {calls} — explicit cancellation could not interrupt the blocking loop"
    );
}

/// Regression: when the turn cap is reached while the model is still emitting tool
/// calls, the async driver must still execute that final round's tools — so the
/// returned `:messages` ends with a correlated `tool_result`, never a dangling
/// assistant `tool_calls` turn (which a follow-up run would feed back and providers
/// reject). Mirrors the blocking `run_tool_loop`, which executes the last round's
/// tools. `max-turns 2` against a 3-tool-round script forces the cap mid-tools.
#[test]
#[serial]
fn round_cap_with_pending_tools_leaves_valid_history() {
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_loop(3, "ping", serde_json::json!({ "n": 1 }), "never-reached")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // Run the agent as a spawned task (async path) and await its result. With
    // max-turns 2 the cap hits on round 2 while a tool call is pending.
    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 2})
        (let ((res (async/await (async/spawn (fn () (agent/run bot "go" {:trace true}))))))
          (let ((msgs (:messages res)))
            ;; The last message must be a correlated tool result (has :tool-call-id),
            ;; proving the final round's tools ran — not a dangling :tool-calls turn.
            (list (not (nil? (get (last msgs) :tool-call-id)))
                  ;; ...and every assistant :tool-calls turn is followed somewhere by a
                  ;; matching tool result (no orphan tool call in the returned history).
                  (every? (fn (m)
                            (or (nil? (get m :tool-calls))
                                (any? (fn (id)
                                        (any? (fn (r) (= (get r :tool-call-id) id)) msgs))
                                      (map (fn (tc) (get tc :id)) (get m :tool-calls)))))
                          msgs))))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("cap-mid-tools agent evaluated");
    let checks = val.as_list().expect("(last-is-tool-result all-correlated)");
    assert_eq!(
        checks[0].as_bool(),
        Some(true),
        "returned :messages must end with a tool result, not a dangling assistant \
         tool_calls turn (the final round's tools were skipped at the cap)"
    );
    assert_eq!(
        checks[1].as_bool(),
        Some(true),
        "every assistant tool_calls turn in the returned history must have a \
         correlated tool result (no orphan tool call)"
    );
}

/// Cancelling an agent mid-loop must not leak its slab entry: the task-reaped
/// sweep (fired at the cancellation transition, since the cancelled task's
/// bytecode never reaches `__agent-finish`) removes the `AGENT_RUNS` entry —
/// messages, tool Values, closures — right then, not at `reset_runtime_state`.
/// And the runtime stays healthy: a subsequent `agent/run` on the same
/// interpreter completes normally and also leaves the slab empty.
#[test]
#[serial]
fn cancelled_agent_leaves_no_slab_entry_and_next_run_works() {
    reset_io_inflight();

    // 8 tool rounds (9 calls) at 100 ms each ⇒ ~900 ms full; cancel at 250 ms.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(100)
        .tool_loop(8, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let cancel_program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 12})
        (let ((p (async/spawn (fn () (agent/run bot "go")))))
          (async/spawn (fn () (async/sleep 250) (async/cancel p)))
          (try (async/await p) (catch e nil)))
    "#;
    let _ = interp.eval_str_compiled(cancel_program);

    assert_eq!(
        agent_runs_len(),
        0,
        "cancelled agent's slab entry must be reaped at the cancellation \
         transition, not leak until reset_runtime_state"
    );

    // A fresh run on the SAME interpreter (same provider script, request-keyed
    // so the new conversation replays from round 1) completes normally.
    let next_program = r#"
        (async/await (async/spawn (fn () (agent/run bot "go"))))
    "#;
    let val = interp
        .eval_str_compiled(next_program)
        .expect("agent/run after a cancelled run must still work");
    assert_eq!(val.as_str(), Some("done"));
    assert_eq!(
        agent_runs_len(),
        0,
        "normal completion after the cancelled run must leave the slab empty"
    );
}

/// The cancelled agent's `invoke_agent` span must be ENDED (exported) by the
/// task-reaped sweep — balanced, on the VM thread — rather than leaking open
/// until teardown (where it was only defused, losing the telemetry entirely).
#[test]
#[serial]
fn cancelled_agent_span_is_exported() {
    let cap = sema_otel::testing::install();
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(100)
        .tool_loop(8, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 12})
        (let ((p (async/spawn (fn () (agent/run bot "go")))))
          (async/spawn (fn () (async/sleep 250) (async/cancel p)))
          (try (async/await p) (catch e nil)))
    "#;
    let _ = interp.eval_str_compiled(program);
    assert_eq!(agent_runs_len(), 0, "slab reaped on cancel");

    let spans = cap.spans_json();
    let agent_spans: Vec<&serde_json::Value> = spans
        .iter()
        .filter(|s| s["attributes"]["gen_ai.operation.name"] == "invoke_agent")
        .collect();
    assert_eq!(
        agent_spans.len(),
        1,
        "the cancelled agent's invoke_agent span must be exported (ended by the \
         reap sweep), got {} agent spans",
        agent_spans.len()
    );
    let status = agent_spans[0]["status"].as_str().unwrap_or_default();
    assert!(
        status.starts_with("error"),
        "the reaped span must carry the cancellation error status, got {status:?}"
    );
}

/// No-regression: the slab is empty after BOTH ordinary exits — a normal
/// completion (driver reaches `__agent-finish`) and an error completion (the
/// consecutive-tool-error abort raised through `__agent-finish`). Neither path
/// depends on the cancel sweep.
#[test]
#[serial]
fn normal_and_error_completion_leave_slab_empty() {
    reset_io_inflight();

    // Normal completion.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_loop(2, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));
    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 6})
        (async/await (async/spawn (fn () (agent/run bot "go"))))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("normal async agent run");
    assert_eq!(val.as_str(), Some("done"));
    assert_eq!(agent_runs_len(), 0, "normal completion must empty the slab");

    // Error completion: 6 queued failing tool rounds trip the consecutive-error
    // abort, raised from `__agent-finish` and propagated through the await.
    let mut b = FakeProvider::builder("fake").model("fake-model");
    for i in 0..6 {
        b = b.tool_call(&format!("c{i}"), "flaky", serde_json::json!({ "x": "bad" }));
    }
    let fake = b.build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));
    let program = r#"
        (deftool flaky "Always fails" {:x {:type :string}}
          (lambda (x) (throw "boom")))
        (defagent bot {:model "fake-model" :tools [flaky] :max-turns 10})
        (async/await (async/spawn (fn () (agent/run bot "go"))))
    "#;
    let err = interp
        .eval_str_compiled(program)
        .expect_err("runaway tool errors must abort the async run too");
    assert!(
        err.to_string().contains("consecutive tool errors"),
        "expected a consecutive-tool-errors abort, got: {err}"
    );
    assert_eq!(agent_runs_len(), 0, "error completion must empty the slab");
}

/// Issue #87: a cancelled `agent/run` must be able to hand back the partial
/// transcript it had assembled. The `:on-partial` callback fires from the
/// driver's cancel catch with the same `{:response :messages :session}` map
/// `__agent-finish` builds, so the interrupted turn keeps its correlated
/// history (assistant tool_calls + tool_result rounds), which `async/await`
/// cannot return for a cancelled task.
#[test]
#[serial]
fn cancelled_agent_run_reports_partial_transcript() {
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(100)
        .tool_loop(8, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 12})
        (define partial nil)
        (let ((p (async/spawn
                   (fn () (agent/run bot "go"
                            {:on-partial (fn (r) (set! partial r))})))))
          (async/spawn (fn () (async/sleep 250) (async/cancel p)))
          (try (async/await p) (catch e nil)))
        (if (nil? partial) 0 (length (:messages partial)))
    "#;
    let n = interp
        .eval_str_compiled(program)
        .expect("program should run")
        .as_int()
        .expect("message count");
    assert!(
        n > 1,
        "the cancelled run must report the transcript it had (user turn + at \
         least one completed round), got {n} messages"
    );
    assert_eq!(agent_runs_len(), 0, "slab still reaped on cancel");
}

#[test]
#[serial]
fn failing_on_partial_callback_does_not_mask_agent_error() {
    let mut fake = FakeProvider::builder("fake").model("fake-model");
    for i in 0..6 {
        fake = fake.tool_call(&format!("c{i}"), "flaky", serde_json::json!({ "x": "bad" }));
    }

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake.build()));
    let error = interp
        .eval_str_compiled(
            r#"
            (deftool flaky "Always fails" {:x {:type :string}}
              (fn (x) (throw "tool failed")))
            (defagent bot {:model "fake-model" :tools [flaky] :max-turns 10})
            (async/await
              (async/spawn
                (fn () (agent/run bot "go"
                         {:on-partial (fn (partial) (throw "callback failed"))}))))
            "#,
        )
        .expect_err("the agent run must retain its original error");

    assert!(
        error.to_string().contains("consecutive tool errors"),
        "the callback must not replace the agent error: {error}"
    );
    assert_eq!(agent_runs_len(), 0);
}
