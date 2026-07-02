//! BREAKER suite for the non-blocking multi-round `agent/run` (issue #61 §3a, ADR
//! #68). These are adversarial, deterministic FakeProvider tests that stress the
//! Sema-driven loop (`__agent-drive` over `__agent-begin`/`__agent-step`/
//! `__agent-exec-tools`/`__agent-finish`, backed by the thread-local `AGENT_RUNS`
//! slab) under interleaving, cancellation, sub-agent tools, and handle misuse.
//!
//! Everything is keyless and reproducible: a `FakeProvider` with `tool_loop` (a
//! request-keyed multi-round script, deterministic under ANY interleaving) plus an
//! injected `chat_delay` per round to force real overlap on the cooperative
//! scheduler. No network, no wall-clock sleeps for correctness (only for overlap
//! timing). Own binary — the `IO_INFLIGHT` atomics, provider registry, and otel
//! exporter are process-global, so these `#[serial]` tests must not share a process
//! with unrelated inflight/span capture.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_llm::builtins::{
    io_peak_inflight, register_test_provider, reset_io_inflight, reset_runtime_state,
};
use sema_llm::fake::FakeProvider;
use serial_test::serial;

/// Assert, for a single recorded request, that its tool-call correlation is
/// internally valid: every tool-result message (`role == "tool"`) carries a
/// non-empty `tool_call_id` + `tool_name`, and is preceded (earlier in the same
/// request) by an assistant message whose `tool_calls` include that id. This is the
/// invariant OpenAI-family providers require and the property interleaving must not
/// scramble across agents.
fn assert_request_correlation(req: &sema_llm::types::ChatRequest, ctx: &str) {
    for (i, m) in req.messages.iter().enumerate() {
        if m.role == "tool" {
            let id = m
                .tool_call_id
                .as_deref()
                .unwrap_or_else(|| panic!("{ctx}: tool-result msg #{i} missing tool_call_id"));
            assert!(
                m.tool_name
                    .as_deref()
                    .map(|s| !s.is_empty())
                    .unwrap_or(false),
                "{ctx}: tool-result msg #{i} (id={id}) missing tool_name"
            );
            let has_prior_call = req.messages[..i].iter().any(|prev| {
                prev.role == "assistant" && prev.tool_calls.iter().any(|tc| tc.id == id)
            });
            assert!(
                has_prior_call,
                "{ctx}: tool-result (id={id}) has NO preceding assistant tool_calls turn — \
                 correlation was scrambled",
            );
        }
    }
}

/// (1) Tool-call correlation UNDER INTERLEAVING. Three concurrent multi-round
/// agents (each 2 tool rounds + a final reply) with distinct user markers. Because
/// the slab is keyed per agent token and messages never leave Rust, every recorded
/// request must stay internally correlated AND belong to exactly one agent. The
/// per-agent length progression [1,3,5] is the anti-scramble oracle: if a round's
/// assistant/tool_result landed in the wrong agent's slab, a request length or a
/// marker grouping would be off.
#[test]
#[serial]
fn correlation_holds_under_interleaving() {
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(40)
        .tool_loop(2, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();
    let recorder = fake.recorder();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // Three agents with DISTINCT user inputs, spawned concurrently.
    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 6})
        (async/all
          (map (fn (marker) (async/spawn (fn () (agent/run bot marker))))
               (list "alpha" "bravo" "charlie")))
    "#;
    interp
        .eval_str_compiled(program)
        .expect("3 concurrent agents evaluated");

    // Overlap actually happened (else the test would be vacuous).
    assert!(
        io_peak_inflight() >= 2,
        "expected peak in-flight >= 2 (agents overlapping), got {}",
        io_peak_inflight()
    );

    let reqs = recorder.requests();
    assert_eq!(
        reqs.len(),
        9,
        "3 agents x (2 tool rounds + 1 final) = 9 provider requests, got {}",
        reqs.len()
    );

    // Every request stays internally correlated regardless of interleaving.
    for (i, req) in reqs.iter().enumerate() {
        assert_request_correlation(req, &format!("request #{i}"));
    }

    // Group requests by their (single) user marker; each agent must have produced
    // the length progression [1,3,5] — proof no round leaked across agents.
    use std::collections::BTreeMap;
    let mut by_marker: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for req in &reqs {
        let users: Vec<String> = req
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.to_text())
            .collect();
        assert_eq!(
            users.len(),
            1,
            "each request must carry exactly one user message (its own agent's), got {users:?}"
        );
        by_marker
            .entry(users[0].clone())
            .or_default()
            .push(req.messages.len());
    }
    assert_eq!(
        by_marker.len(),
        3,
        "expected 3 distinct agent markers, got {:?}",
        by_marker.keys().collect::<Vec<_>>()
    );
    for (marker, mut lens) in by_marker {
        lens.sort_unstable();
        assert_eq!(
            lens,
            vec![1, 3, 5],
            "agent {marker:?} request-length progression scrambled: {lens:?}"
        );
    }
}

/// (2) Usage accounting EXACTLY ONCE PER ROUND under interleaving. Three concurrent
/// agents, each 2 tool rounds + a final reply = 3 provider calls, each 15 tokens
/// (10 prompt + 5 completion). Session total must be exactly 9 * 15 = 135 — no
/// double-count (would be >135), no drop (would be <135).
#[test]
#[serial]
fn usage_accounted_once_per_round_under_interleaving() {
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(40)
        .tool_loop(2, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();
    let recorder = fake.recorder();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 6})
        (async/all
          (map (fn (i) (async/spawn (fn () (agent/run bot "go"))))
               (list 1 2 3)))
        (:total-tokens (llm/session-usage))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("3 concurrent agents accounted");

    assert!(
        io_peak_inflight() >= 2,
        "expected overlap (peak in-flight >= 2), got {}",
        io_peak_inflight()
    );
    assert_eq!(
        recorder.call_count(),
        9,
        "3 agents x 3 rounds = 9 provider calls"
    );
    assert_eq!(
        val.as_int(),
        Some(135),
        "9 rounds x 15 tokens must total 135 (accounted exactly once per round); \
         >135 = double-count, <135 = dropped"
    );
}

/// (3a) OTel SIBLING ISOLATION. Two concurrent agents; each agent's `chat` and
/// `execute_tool` spans must parent under ITS OWN `invoke_agent` span (same trace),
/// never the sibling's — distinct trace roots, no cross-parenting.
#[test]
#[serial]
fn concurrent_agents_spans_are_isolated() {
    let cap = sema_otel::testing::install();
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(60)
        .tool_loop(1, "ping", serde_json::json!({ "n": 1 }), "final")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 4})
        (async/all
          (map (fn (m) (async/spawn (fn () (agent/run bot m))))
               (list "one" "two")))
    "#;
    interp
        .eval_str_compiled(program)
        .expect("two concurrent agents evaluated");

    let spans = cap.spans_json();
    let agents: Vec<&serde_json::Value> = spans
        .iter()
        .filter(|s| s["attributes"]["gen_ai.operation.name"] == "invoke_agent")
        .collect();
    assert_eq!(agents.len(), 2, "expected two invoke_agent spans");

    let a0 = agents[0]["span_id"].as_str().unwrap();
    let a1 = agents[1]["span_id"].as_str().unwrap();
    let t0 = agents[0]["trace_id"].as_str().unwrap();
    let t1 = agents[1]["trace_id"].as_str().unwrap();
    assert_ne!(a0, a1, "distinct agent span ids");
    assert_ne!(t0, t1, "each spawned agent is its own root trace");

    // Every chat / execute_tool span parents under the agent that shares its trace,
    // and NEVER under the sibling agent's span.
    for s in &spans {
        let op = &s["attributes"]["gen_ai.operation.name"];
        if op != "chat" && op != "execute_tool" {
            continue;
        }
        let trace = s["trace_id"].as_str().unwrap();
        let parent = s["parent_span_id"].as_str().unwrap_or("");
        let (own_agent, sibling_agent) = if trace == t0 {
            (a0, a1)
        } else if trace == t1 {
            (a1, a0)
        } else {
            panic!("span in an unknown trace {trace}");
        };
        assert_eq!(
            parent, own_agent,
            "{op} span must parent under its own agent (trace {trace})"
        );
        assert_ne!(
            parent, sibling_agent,
            "{op} span cross-parented under sibling agent"
        );
    }
}

/// (3b) Cancel ONE of two concurrent agents mid-run; the survivor's result and
/// spans must stay intact (no trace corruption, no panic). The Drop-on-cancel of
/// the cancelled agent's slab state pops+ends its span; a mis-pop would corrupt the
/// survivor's active-span stack. Invariant checked globally: every EXPORTED chat
/// span parents under the invoke_agent span in its own trace.
#[test]
#[serial]
fn cancelling_one_agent_leaves_survivor_spans_intact() {
    let cap = sema_otel::testing::install();
    reset_io_inflight();

    // Both agents share the tool_loop(6) provider. longbot (max-turns 10) is
    // cancelled mid-run at 200ms; the survivor (max-turns 8) is awaited separately
    // (no timeout) so it runs all 6 tool rounds + the "final" reply to completion.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(60)
        .tool_loop(6, "ping", serde_json::json!({ "n": 1 }), "final")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // Spawn both; cancel the long one at 200ms; await the survivor's full result.
    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent longbot  {:model "fake-model" :tools [ping] :max-turns 10})
        (defagent survivor {:model "fake-model" :tools [ping] :max-turns 8})
        (define longp (async/spawn (fn () (agent/run longbot  "cancel-me"))))
        (define survp (async/spawn (fn () (agent/run survivor "survive"))))
        (try (async/timeout 200 longp) (catch e nil))
        (async/await survp)
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("survivor eval must not panic despite the cancel");

    // The survivor runs 6 tool rounds then the final reply — the cancel of its
    // sibling must not corrupt its result.
    assert_eq!(
        val.as_str(),
        Some("final"),
        "survivor agent must complete with the correct final answer, got {val:?}"
    );

    // Global trace well-formedness: every exported chat span parents under the
    // invoke_agent span sharing its trace. A cancel-driven mis-pop would break this.
    let spans = cap.spans_json();
    use std::collections::HashMap;
    let agent_by_trace: HashMap<String, String> = spans
        .iter()
        .filter(|s| s["attributes"]["gen_ai.operation.name"] == "invoke_agent")
        .map(|s| {
            (
                s["trace_id"].as_str().unwrap().to_string(),
                s["span_id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    for s in &spans {
        if s["attributes"]["gen_ai.operation.name"] != "chat" {
            continue;
        }
        let trace = s["trace_id"].as_str().unwrap();
        let parent = s["parent_span_id"].as_str().unwrap_or("");
        if let Some(agent_id) = agent_by_trace.get(trace) {
            assert_eq!(
                parent, agent_id,
                "a chat span in trace {trace} parents under {parent}, not its agent {agent_id} \
                 — the cancel corrupted the trace"
            );
        }
    }
}

/// (4) Async SUB-AGENT / yielding TOOL. A `deftool` whose body calls `llm/complete`
/// runs inside `__agent-exec-tools` in ordinary task context, so the sub-completion
/// must be able to yield. Assert (a) the outer agent completes with the correct
/// final answer, (b) the provider was called exactly 3 times (round1 tool call, the
/// tool's sub-completion, round2 final), and (c) the tool body observed it was in
/// async context (so the sub-call went through the yielding path, not a degraded
/// blocking one).
#[test]
#[serial]
fn yielding_tool_calls_llm_complete_in_task_context() {
    reset_io_inflight();

    // Scripted (single outer agent ⇒ deterministic call order): round1 tool call,
    // the tool's own llm/complete, then round2 final.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(30)
        .tool_call("call_1", "subcall", serde_json::json!({ "q": "hi" }))
        .reply("sub-result")
        .reply("final-answer")
        .build();
    let recorder = fake.recorder();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (define ctx-ch (channel/new 8))
        (deftool subcall "calls the model" {:q {:type :string}}
          (fn (q)
            (channel/send ctx-ch (__async-context?))
            (llm/complete "please")))
        (defagent bot {:model "fake-model" :tools [subcall] :max-turns 4})
        (define result
          (first (async/all (list (async/spawn (fn () (agent/run bot "go")))))))
        (list result (channel/try-recv ctx-ch))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("outer agent with a yielding tool evaluated");

    let outer = val.as_list().expect("(result async-ctx?)");
    assert_eq!(
        outer[0].as_str(),
        Some("final-answer"),
        "outer agent must complete with the round-2 final answer"
    );
    assert_eq!(
        outer[1].as_bool(),
        Some(true),
        "the tool body must run in async task context (sub-completion took the yielding path)"
    );
    assert_eq!(
        recorder.call_count(),
        3,
        "expected exactly 3 provider calls: round1 tool call, the tool's llm/complete, round2 final"
    );
}

/// (4b) Nested `agent/run` inside a tool. Exercises slab REENTRANCY: two live
/// tokens (outer + inner) on the same thread-local slab at once. The inner agent
/// runs in the outer tool's task context; both must complete cleanly.
#[test]
#[serial]
fn nested_agent_run_inside_tool_is_reentrant() {
    reset_io_inflight();

    // Outer: round1 tool call -> tool runs inner agent -> round2 final.
    // Inner: a single plain reply (no tools) so it terminates in one round.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(20)
        .tool_call("outer_1", "delegate", serde_json::json!({ "q": "x" }))
        .reply("inner-done") // inner agent's single round
        .reply("outer-done") // outer round2
        .build();
    let recorder = fake.recorder();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (defagent inner {:model "fake-model" :max-turns 3})
        (deftool delegate "delegates to a sub-agent" {:q {:type :string}}
          (fn (q) (agent/run inner "sub")))
        (defagent outer {:model "fake-model" :tools [delegate] :max-turns 4})
        (first (async/all (list (async/spawn (fn () (agent/run outer "go"))))))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("nested agent/run inside a tool evaluated");
    assert_eq!(
        val.as_str(),
        Some("outer-done"),
        "outer agent must complete after the nested inner agent ran as its tool"
    );
    assert_eq!(
        recorder.call_count(),
        3,
        "outer round1 + inner round + outer round2 = 3 provider calls"
    );
}

/// (5) CANCELLATION correctness. Cancel an agent mid-loop; assert no panic, a
/// sibling task is unaffected, and a SUBSEQUENT `agent/run` in the same interpreter
/// (also on the slab path) still works — proving the slab is not corrupted by the
/// cancel's Drop.
#[test]
#[serial]
fn cancel_then_subsequent_agent_run_still_works() {
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(80)
        .tool_loop(8, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 12})
        (define sib (async/spawn (fn () (begin (async/sleep 20) "sib-ok"))))
        (define a   (async/spawn (fn () (agent/run bot "go"))))
        ;; Cancel the long agent ~250ms in; swallow the timeout throw.
        (try (async/timeout 250 a) (catch e nil))
        (let ((sib-res  (async/await sib))
              (next-res  (first (async/all (list (async/spawn (fn () (agent/run bot "again"))))))))
          (list sib-res next-res))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("cancel + sibling + subsequent-run eval must not panic");

    let outer = val.as_list().expect("(sib-res next-res)");
    assert_eq!(
        outer[0].as_str(),
        Some("sib-ok"),
        "sibling task must be unaffected by the agent cancel"
    );
    assert_eq!(
        outer[1].as_str(),
        Some("done"),
        "a subsequent agent/run on the slab path must still complete (slab not corrupted)"
    );
}

/// (6) HANDLE MISUSE. Calling the internal `__agent-*` natives with a bogus / bad
/// token must produce a clean Sema error (or a defined no-op), never a panic/UB.
#[test]
#[serial]
fn agent_handle_misuse_is_a_clean_error() {
    let fake = FakeProvider::builder("fake").model("fake-model").build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // Bogus integer token: step/exec-tools must Err (handle not found).
    assert!(
        interp.eval_str_compiled("(__agent-step 999999)").is_err(),
        "__agent-step with an unknown token must be a clean error"
    );
    assert!(
        interp
            .eval_str_compiled("(__agent-exec-tools 999999)")
            .is_err(),
        "__agent-exec-tools with an unknown token must be a clean error"
    );
    // __agent-finish is idempotent: an unknown token is a defined no-op (nil).
    let fin = interp
        .eval_str_compiled("(__agent-finish 999999)")
        .expect("__agent-finish on an unknown token is a no-op, not an error");
    assert!(fin.is_nil(), "__agent-finish on unknown token returns nil");

    // Non-integer / negative token: type error, not a panic.
    assert!(
        interp
            .eval_str_compiled(r#"(__agent-step "not-an-int")"#)
            .is_err(),
        "__agent-step with a non-integer token must be a clean type error"
    );
    assert!(
        interp.eval_str_compiled("(__agent-step -1)").is_err(),
        "__agent-step with a negative token must be a clean type error"
    );
    // Arity misuse.
    assert!(
        interp.eval_str_compiled("(__agent-step)").is_err(),
        "__agent-step with no args must be a clean arity error"
    );
}

/// (7) RESULT-SHAPE PARITY. The same agent run must return the SAME shape/content
/// whether driven async (spawned task, slab path) or blocking (top-level). Checks
/// both the 2-arg string form and the 3-arg `{:response :messages :session}` map.
#[test]
#[serial]
fn async_and_blocking_result_shapes_match() {
    reset_io_inflight();

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_loop(1, "ping", serde_json::json!({ "n": 1 }), "answer")
        .build();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));

    // String form: blocking (top-level) vs async (spawned).
    let program = r#"
        (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
        (defagent bot {:model "fake-model" :tools [ping] :max-turns 4})
        (define blocking-str (agent/run bot "q"))
        (define async-str
          (first (async/all (list (async/spawn (fn () (agent/run bot "q")))))))
        ;; Map form (3-arg with opts).
        (define blocking-map (agent/run bot "q" {}))
        (define async-map
          (first (async/all (list (async/spawn (fn () (agent/run bot "q" {})))))))
        (list blocking-str async-str
              (:response blocking-map) (:response async-map)
              (list? (:messages blocking-map)) (list? (:messages async-map))
              (length (:messages blocking-map)) (length (:messages async-map))
              (nil? (:session blocking-map)) (nil? (:session async-map)))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("parity program evaluated");
    let r = val.as_list().expect("parity result list");

    // (a) String form identical.
    assert_eq!(r[0].as_str(), Some("answer"), "blocking string form");
    assert_eq!(
        r[0].as_str(),
        r[1].as_str(),
        "async string form must equal blocking string form"
    );
    // (b) Map :response identical.
    assert_eq!(r[2].as_str(), Some("answer"), "blocking map :response");
    assert_eq!(
        r[2].as_str(),
        r[3].as_str(),
        "async map :response must equal blocking map :response"
    );
    // (c) :messages is a list in both.
    assert_eq!(r[4].as_bool(), Some(true), "blocking :messages is a list");
    assert_eq!(r[5].as_bool(), Some(true), "async :messages is a list");
    // (d) same number of messages.
    assert_eq!(
        r[6].as_int(),
        r[7].as_int(),
        "async and blocking must build the same number of messages, got {:?} vs {:?}",
        r[6].as_int(),
        r[7].as_int()
    );
    // (e) :session present in both.
    assert_eq!(r[8].as_bool(), Some(false), "blocking :session present");
    assert_eq!(r[9].as_bool(), Some(false), "async :session present");
}

/// (obs) `:on-text` in an ASYNC agent context does NOT error — it silently takes
/// the synchronous inline streaming path (`do_complete_streaming` on the VM thread),
/// which blocks siblings for that round (the documented honest-limit). The plan's
/// "on-text is validated synchronous-only" is aspirational: there is no rejection,
/// just a graceful degrade to inline. This guard pins that it still completes
/// correctly and delivers the streamed deltas.
#[test]
#[serial]
fn on_text_in_async_runs_inline_and_completes() {
    reset_io_inflight();
    // stream_complete pops the script queue (tool_loop only drives complete()).
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .stream(&["hel", "lo"])
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));
    let program = r#"
        (define acc (channel/new 16))
        (defagent bot {:model "fake-model" :max-turns 4})
        (define res
          (first (async/all (list (async/spawn (fn ()
            (agent/run bot "go" {:on-text (fn (delta) (channel/send acc delta))})))))))
        (define (drain ch acc)
          (let ((v (channel/try-recv ch)))
            (if (nil? v) acc (drain ch (string-append acc v)))))
        (list (:response res) (drain acc ""))
    "#;
    let val = interp
        .eval_str_compiled(program)
        .expect("on-text in async agent must complete (inline streaming), not error");
    let r = val.as_list().expect("(response streamed)");
    assert_eq!(
        r[0].as_str(),
        Some("hello"),
        "on-text async agent must return the streamed final content"
    );
    assert_eq!(
        r[1].as_str(),
        Some("hello"),
        "the on-text callback must receive the streamed deltas (delivered inline)"
    );
}
