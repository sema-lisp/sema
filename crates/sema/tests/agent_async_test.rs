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
//! proven three ways: peak offloaded futures in flight ≥ 2, a max-not-sum wall
//! clock, and a sibling ticker that advances *during* the agent's rounds.
//!
//! Own binary — the `IO_INFLIGHT` instrumentation atomics and the provider registry
//! are process-global, so these `#[serial]` tests must not share a process with
//! unrelated inflight capture.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{
    io_peak_inflight, register_test_provider, reset_io_inflight, reset_runtime_state,
};
use sema_llm::fake::FakeProvider;
use serial_test::serial;

/// N agents, each a 2-tool-round conversation (3 provider calls) with a 120 ms
/// delay per round, spawned concurrently. Blocking today: each agent hogs the VM
/// thread through all its rounds, so they run serially (peak in-flight = 1, wall ≈
/// N·3·120). Non-blocking: every round offloads + yields, so the agents overlap
/// (peak in-flight ≥ 2, wall ≈ 3·120 — the single-agent critical path).
#[test]
#[serial]
#[ignore = "RED acceptance gate for non-blocking multi-round agent/run (issue #61 §3a); un-ignored when the yield-per-round implementation lands"]
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

    // 3 agents × (2 tool rounds + 1 final) × 120 ms:
    //   serial floor ≈ 1080 ms; overlapped ≈ 360 ms.
    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 6})
        (let ((t0 (sys/elapsed)))
          (async/all
            (map (fn (i) (async/spawn (fn () (agent/run bot "go"))))
                 (list 1 2 3)))
          (floor (/ (- (sys/elapsed) t0) 1000000)))
    "#;
    let wall = interp
        .eval_str_compiled(program)
        .expect("3 concurrent agents evaluated");
    let wall_ms = wall.as_int().expect("wall ms");

    assert!(
        io_peak_inflight() >= 2,
        "expected peak offloaded futures in flight >= 2 (agents overlapping across \
         rounds), got {} — agent/run still blocks per round",
        io_peak_inflight()
    );
    assert!(
        wall_ms < 700,
        "expected overlapped wall < 700 ms (serial floor ~1080 ms), got {wall_ms} ms"
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
#[ignore = "RED acceptance gate for non-blocking multi-round agent/run (issue #61 §3a); un-ignored when the yield-per-round implementation lands"]
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

/// Cancelling a running agent (via `async/timeout` shorter than the full
/// conversation) must cut the round loop short: no NEW provider round starts after
/// the cutoff. Blocking today: `agent/run` is one synchronous native call the
/// scheduler cannot interrupt, so the timeout can't fire until the agent has
/// already run every round — all `full` provider calls happen. Non-blocking: the
/// agent parks on `AwaitIo` between rounds, the timeout cancels it there, and the
/// remaining rounds never dispatch (best-effort for the in-flight round).
#[test]
#[serial]
#[ignore = "RED acceptance gate for non-blocking multi-round agent/run (issue #61 §3a); un-ignored when the yield-per-round implementation lands"]
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
          ;; (async/timeout ms promise): cancel the agent ~250 ms in. Swallow the
          ;; timeout throw — we assert on how many rounds dispatched, not the result.
          (try (async/timeout 250 p) (catch e nil)))
    "#;
    let _ = interp.eval_str_compiled(program);

    let calls = recorder.call_count();
    assert!(
        calls > 0 && calls < 9,
        "expected the cancelled agent to stop short of all 9 provider rounds, but it \
         made {calls} — the timeout could not interrupt the blocking loop"
    );
}
