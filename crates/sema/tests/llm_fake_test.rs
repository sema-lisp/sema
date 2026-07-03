//! Deterministic, key-free tests of the LLM/agent paths using a scripted
//! `FakeProvider`. This is the regression oracle for the agent tool loop —
//! including the round-2 tool-result message shape that the Phase 2 fix targets.
//!
//! No network, no API keys: `register_test_provider` installs the fake as the
//! default provider into the thread-local registry the runtime reads from.

use std::sync::Arc;

use sema_core::Value;
use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::{FakeProvider, FakeRecorder};

/// Build an interpreter, install `fake` as the default provider, run `src`.
/// Returns the eval result plus the recorder handle for asserting on the exact
/// requests the runtime built.
fn eval_with_fake(
    src: &str,
    fake: FakeProvider,
) -> (Result<Value, sema_core::SemaError>, Arc<FakeRecorder>) {
    let interp = Interpreter::new();
    // Fresh provider state, then install the fake as default.
    reset_runtime_state();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    let result = interp.eval_str_compiled(src);
    (result, recorder)
}

#[test]
fn llm_complete_returns_scripted_text() {
    let fake = FakeProvider::builder("fake").reply("hello there").build();
    let (result, recorder) = eval_with_fake(r#"(llm/complete "say hi")"#, fake);
    let val = result.expect("llm/complete should succeed against the fake");
    assert_eq!(val.as_str(), Some("hello there"));
    assert_eq!(recorder.call_count(), 1);
}

#[test]
fn agent_loop_completes_with_tool_call() {
    // Round 1: the model emits a tool call. Round 2 (after the tool result is fed
    // back): the model returns the final answer.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "get-weather", serde_json::json!({"city": "Oslo"}))
        .reply("It is sunny in Oslo.")
        .build();

    let src = r#"
        (deftool get-weather
          "Get current weather for a city"
          {:city {:type :string :description "City name"}}
          (lambda (city)
            (format "{\"city\": \"~a\", \"temp\": 22, \"condition\": \"sunny\"}" city)))

        (defagent weather-bot
          {:model "fake-model"
           :system "You are a weather assistant. Use tools. Be concise."
           :tools [get-weather]
           :max-turns 5})

        (agent/run weather-bot "What's the weather in Oslo?")
    "#;

    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("agent/run should complete against the fake");

    // The agent loop ran two provider rounds and returned the final answer.
    assert_eq!(val.as_str(), Some("It is sunny in Oslo."));
    assert_eq!(
        recorder.call_count(),
        2,
        "expected exactly 2 provider rounds (tool call, then final answer)"
    );

    // Round 2 must carry MORE messages than round 1 — i.e. the assistant turn and
    // the tool result were fed back into history. (The strict correlation check —
    // that the tool result is a correlated `role:tool` / tool_result message with
    // a tool_call_id — is asserted in `agent_loop_round2_is_correlated` once the
    // Phase 2 message model lands.)
    let reqs = recorder.requests();
    assert_eq!(reqs.len(), 2);
    assert!(
        reqs[1].messages.len() > reqs[0].messages.len(),
        "round 2 should include the fed-back tool result"
    );
}

/// Strict oracle for the Phase 2 tool-result protocol fix: round 2 must echo the
/// assistant's tool_calls and send the result as a correlated tool message
/// (`tool_call_id` matching the call). This is what OpenAI-family providers
/// require; before the fix the loop stuffed the result into plain user text with
/// no correlation, so the same agent looped to max-turns and returned empty.
#[test]
fn agent_loop_round2_is_correlated() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "get-weather", serde_json::json!({"city": "Oslo"}))
        .reply("It is sunny in Oslo.")
        .build();

    let src = r#"
        (deftool get-weather "Get weather"
          {:city {:type :string}}
          (lambda (city) "sunny"))
        (defagent weather-bot
          {:model "fake-model" :tools [get-weather] :max-turns 5})
        (agent/run weather-bot "weather in Oslo?")
    "#;

    let (result, recorder) = eval_with_fake(src, fake);
    result.expect("agent/run should complete");

    let reqs = recorder.requests();
    let round2 = &reqs[1];
    // An assistant message echoing the tool_calls...
    assert!(
        round2
            .messages
            .iter()
            .any(|m| m.role == "assistant" && !m.tool_calls.is_empty()),
        "round 2 must echo the assistant's tool_calls"
    );
    // ...followed by a correlated tool-result message keyed by the call id.
    assert!(
        round2
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("call_1")),
        "round 2 must include a tool result correlated by tool_call_id"
    );
}

// ── Phase 3: recoverable tool errors + argument validation ──────────────────

/// A handler that throws on round 1 must NOT abort the run; the error is fed back
/// and the model recovers on round 2.
#[test]
fn tool_handler_error_is_recoverable() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("c1", "flaky", serde_json::json!({"x": "bad"}))
        .reply("recovered")
        .build();
    let src = r#"
        (deftool flaky "A flaky tool" {:x {:type :string}}
          (lambda (x) (if (= x "bad") (throw "boom") "ok")))
        (defagent bot {:model "fake-model" :tools [flaky] :max-turns 5})
        (agent/run bot "use the tool")
    "#;
    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("agent/run must not abort on a tool error");
    assert_eq!(val.as_str(), Some("recovered"));
    assert_eq!(
        recorder.call_count(),
        2,
        "loop should continue after the tool error"
    );
}

/// A wrong-typed argument is rejected by schema validation (before the handler
/// runs), fed back, and the model retries successfully.
#[test]
fn tool_arg_validation_is_recoverable() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("c1", "calc", serde_json::json!({"n": "not-a-number"}))
        .reply("validated-ok")
        .build();
    let src = r#"
        (deftool calc "Needs a number" {:n {:type :number}}
          (lambda (n) (str n)))
        (defagent bot {:model "fake-model" :tools [calc] :max-turns 5})
        (agent/run bot "call calc")
    "#;
    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("agent/run must not abort on an arg validation error");
    assert_eq!(val.as_str(), Some("validated-ok"));
    assert_eq!(recorder.call_count(), 2);
}

/// Runaway error loops are bounded: 5 consecutive failing tool calls abort.
#[test]
fn consecutive_tool_errors_abort() {
    let mut b = FakeProvider::builder("fake").model("fake-model");
    for i in 0..6 {
        b = b.tool_call(&format!("c{i}"), "flaky", serde_json::json!({"x": "bad"}));
    }
    let fake = b.build();
    let src = r#"
        (deftool flaky "Always fails" {:x {:type :string}}
          (lambda (x) (throw "boom")))
        (defagent bot {:model "fake-model" :tools [flaky] :max-turns 10})
        (agent/run bot "go")
    "#;
    let (result, _recorder) = eval_with_fake(src, fake);
    let err = result.expect_err("runaway tool errors must abort");
    assert!(
        err.to_string().contains("consecutive tool errors"),
        "expected a consecutive-tool-errors abort, got: {err}"
    );
}

// ── Phase 4: network resilience (retry/backoff) ─────────────────────────────

use sema_llm::builtins::set_retry_base_ms;
use sema_llm::types::LlmError;

/// A transient 5xx is retried (broadened beyond 429) and the next attempt
/// succeeds. Backoff base is zeroed so the test asserts on attempt count, no sleep.
#[test]
fn transient_5xx_is_retried() {
    let fake = FakeProvider::builder("fake")
        .error(LlmError::Api {
            status: 503,
            message: "service unavailable".into(),
        })
        .reply("after-retry")
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    set_retry_base_ms(0); // no real sleeping
    register_test_provider(Box::new(fake));
    let val = interp
        .eval_str_compiled(r#"(llm/complete "hi")"#)
        .expect("should succeed after retrying the 5xx");
    assert_eq!(val.as_str(), Some("after-retry"));
    assert_eq!(recorder.call_count(), 2, "expected one retry after the 5xx");
}

/// A rate-limit (429) is retried too.
#[test]
fn rate_limit_is_retried() {
    let fake = FakeProvider::builder("fake")
        .error(LlmError::RateLimited { retry_after_ms: 1 })
        .reply("ok")
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    set_retry_base_ms(0);
    register_test_provider(Box::new(fake));
    let val = interp.eval_str_compiled(r#"(llm/complete "hi")"#).unwrap();
    assert_eq!(val.as_str(), Some("ok"));
    assert_eq!(recorder.call_count(), 2);
}

// ── Cache: a hit must not re-charge usage/cost or call the provider ─────────

#[test]
fn cache_hit_does_not_recharge_usage() {
    // One scripted reply only: if the cache served the 2nd call from the provider
    // it would error ("no scripted reply left"). The hit must serve from cache.
    let fake = FakeProvider::builder("fake")
        .reply_with_usage("cached!", 100, 50)
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));
    let src = r#"
        (llm/cache-clear)
        (llm/with-cache {:ttl 3600}
          (fn () (llm/complete "q") (llm/complete "q")))
        (:total-tokens (llm/session-usage))
    "#;
    let val = interp
        .eval_str_compiled(src)
        .expect("cached run should succeed");
    // One real call = 150 tokens; the cache hit must add 0 (not double to 300).
    assert_eq!(val.as_int(), Some(150), "cache hit must not re-count usage");
    assert_eq!(
        recorder.call_count(),
        1,
        "cache hit must be served without calling the provider"
    );
}

/// Prompt-cache token counts flow through to session/last usage. Providers that
/// surface `cache_read_input_tokens` / `cache_creation_input_tokens` (Anthropic
/// distinctly, OpenAI/Gemini as a subset of prompt tokens) must be visible to
/// Sema code via `:cache-read-tokens` / `:cache-creation-tokens`.
#[test]
fn cache_tokens_flow_to_usage() {
    let fake = FakeProvider::builder("fake")
        .reply_with_cache_usage("first", 100, 20, 0, 80)
        .reply_with_cache_usage("second", 100, 20, 80, 0)
        .build();
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));
    let src = r#"
        (llm/complete "q1")
        (llm/complete "q2")
        [(:cache-read-tokens (llm/session-usage))
         (:cache-creation-tokens (llm/session-usage))
         (:cache-read-tokens (llm/last-usage))
         (:cache-creation-tokens (llm/last-usage))]
    "#;
    let val = interp.eval_str_compiled(src).expect("run should succeed");
    let items = val.as_seq().expect("vector result");
    // Session totals accumulate both calls: read = 0+80, creation = 80+0.
    assert_eq!(items[0].as_int(), Some(80), "session cache-read-tokens");
    assert_eq!(items[1].as_int(), Some(80), "session cache-creation-tokens");
    // Last-usage reflects only the 2nd call (80 read, 0 creation).
    assert_eq!(items[2].as_int(), Some(80), "last cache-read-tokens");
    assert_eq!(items[3].as_int(), Some(0), "last cache-creation-tokens");
}

/// A non-retryable 4xx (e.g. 400) is NOT retried — it fails immediately.
#[test]
fn client_4xx_is_not_retried() {
    let fake = FakeProvider::builder("fake")
        .error(LlmError::Api {
            status: 400,
            message: "bad request".into(),
        })
        .reply("never-reached")
        .build();
    let recorder = fake.recorder();
    let interp = Interpreter::new();
    reset_runtime_state();
    set_retry_base_ms(0);
    register_test_provider(Box::new(fake));
    let result = interp.eval_str_compiled(r#"(llm/complete "hi")"#);
    assert!(result.is_err(), "a 400 must not be retried");
    assert_eq!(recorder.call_count(), 1, "no retry on a 4xx");
}

/// Regression guard for the sema-llm eval-callback consolidation
/// (docs/plans/2026-06-22-unify-sema-llm-eval-callback.md): a tool handler that
/// uses real special forms (`let`/`if`/`cond`/`string-append`) and a `set!`
/// side effect on an outer binding must run correctly through whatever evaluator
/// path the agent loop dispatches the handler on. Pins behavior across the
/// migration from the bespoke `call_value_fn` to `sema_core::call_callback`.
#[test]
fn tool_handler_runs_full_evaluator_with_side_effects() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("c1", "classify", serde_json::json!({"n": 7}))
        .reply("done")
        .build();

    let src = r#"
        (define hits 0)
        (deftool classify
          "Classify a number"
          {:n {:type :number}}
          (lambda (n)
            (set! hits (+ hits 1))
            (let ((label (cond ((< n 0) "neg") ((= n 0) "zero") (else "pos"))))
              (if (> n 5) (string-append label "-big") label))))

        (defagent bot {:model "fake-model" :tools [classify] :max-turns 5})
        (agent/run bot "classify 7")
        hits
    "#;

    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("agent/run with a computing tool handler should succeed");
    // The handler's `set!` on the outer `hits` was observed exactly once — proving
    // the handler ran on the full evaluator (let/if/cond all evaluated) and its
    // mutation propagated out through the callback path.
    assert_eq!(
        val.as_int(),
        Some(1),
        "tool handler's set! on an outer binding must persist"
    );
    assert_eq!(recorder.call_count(), 2, "tool call round + final answer");
}

/// CORE-2 mid-agent-loop reclamation (`docs/plans/2026-07-02-core2-gc.md`
/// §5.2): a long agent run must reclaim BETWEEN tool turns, not only when
/// the whole eval returns. Turn 1's handler builds 700 garbage
/// recursive-closure cycles and then churns 3000 dead channels; the channel
/// births cross the registry-growth threshold inside the handler, so the
/// data-birth trigger (`register_candidate`) severs the cycles and prunes
/// the dead entries mid-turn. The turn-boundary `maybe_collect` stays a
/// threshold-gated backstop that the growth policy keeps quiescent here
/// (registry-at-rest never exceeds the threshold once births self-collect).
/// Turn 2's handler observes the outcome as its FIRST action: bounded
/// registry, a real prune, and an explicit collect that finds no garbage
/// cycle left. Message correlation must be unchanged by passes running
/// inside a tool handler. Deterministic — no timing; the fake scripts
/// every round.
#[test]
fn agent_turn_boundary_collects_between_tool_turns() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "churn", serde_json::json!({"x": "one"}))
        .tool_call("call_2", "churn", serde_json::json!({"x": "two"}))
        .reply("all done")
        .build();

    let src = r#"
        (define turn 0)
        (define probe nil)
        (define leftover nil)
        (define (mk-cycle k)
          (define (r n) (if (<= n 0) k (r (- n 1))))
          r)
        (define (spin i)
          (if (<= i 0) nil (begin (mk-cycle i) (spin (- i 1)))))
        (define (spam-channels i)
          (if (<= i 0) nil (begin (channel/new 1) (spam-channels (- i 1)))))
        (deftool churn "Churn the heap" {:x {:type :string}}
          (lambda (x)
            (set! turn (+ turn 1))
            (if (= turn 1)
                (begin (spin 700) (spam-channels 3000) "turn-1 done")
                (begin (set! probe (gc/stats))
                       (set! leftover (gc/collect))
                       "turn-2 done"))))
        (defagent bot {:model "fake-model" :tools [churn] :max-turns 5})
        (define answer (agent/run bot "go"))
        (list answer
              (< (:registry-size probe) 1024)
              (>= (:pruned probe) 900)
              (:collected leftover))
    "#;

    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("agent run with a churning tool handler should complete");
    let items = val.as_seq().expect("result list");
    // The loop completed correctly across the collections...
    assert_eq!(items[0].as_str(), Some("all done"));
    // ...and turn 2 observed the mid-turn passes: registry bounded well
    // under the spam count, the last pass really pruned a dead batch, and
    // the 700 garbage cycles were already severed before turn 2 started
    // (the explicit collect has nothing left to reclaim).
    assert_eq!(
        items[1],
        Value::bool(true),
        "registry must be pruned before turn 2 (probe below 1024)"
    );
    assert_eq!(
        items[2],
        Value::bool(true),
        "the last mid-turn pass must have pruned a dead-entry batch"
    );
    assert_eq!(
        items[3],
        Value::int(0),
        "all garbage cycles must be reclaimed before turn 2's explicit collect"
    );

    // Zero behavior change to messages/correlation: 3 provider rounds, each
    // tool round echoed the assistant tool_calls turn and fed back a
    // correlated tool result.
    let reqs = recorder.requests();
    assert_eq!(reqs.len(), 3, "two tool rounds + the final answer");
    let round2 = &reqs[1];
    assert!(
        round2
            .messages
            .iter()
            .any(|m| m.role == "assistant" && !m.tool_calls.is_empty()),
        "round 2 must echo the assistant's tool_calls"
    );
    assert!(
        round2
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("call_1")),
        "round 2 must include the tool result correlated to call_1"
    );
    let round3 = &reqs[2];
    assert!(
        round3
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("call_2")),
        "round 3 must include the tool result correlated to call_2"
    );
    assert!(
        round3.messages.len() > round2.messages.len(),
        "history must keep growing across turns"
    );
}

#[test]
fn rerank_reorders_documents_by_relevance() {
    // Three candidates; the fake scripts a reordering: doc index 2 most relevant,
    // then 0, then 1 — each with a descending relevance score.
    let fake = FakeProvider::builder("fake")
        .model("rerank-test")
        .rerank(&[(2, 0.91), (0, 0.42), (1, 0.10)])
        .build();
    let src = r#"
        (llm/rerank "how do I read a file?"
          (list "vectors are cool" "unrelated trivia" "use file/read to read a file")
          {:top-k 3})
    "#;
    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("llm/rerank should succeed");
    let items = val.as_seq().expect("rerank returns a list");
    assert_eq!(items.len(), 3);

    // Top result is the original index 2 (the actually-relevant doc), with its score.
    let top = items[0].as_map_rc().expect("result is a map");
    assert_eq!(
        top.get(&Value::keyword("index")).and_then(|v| v.as_int()),
        Some(2)
    );
    assert_eq!(
        top.get(&Value::keyword("document"))
            .and_then(|v| v.as_str()),
        Some("use file/read to read a file")
    );
    let score = top
        .get(&Value::keyword("score"))
        .and_then(|v| v.as_float())
        .unwrap();
    assert!((score - 0.91).abs() < 1e-9);

    // The runtime forwarded the canonical request: query + all three documents + top_k.
    let reqs = recorder.reranks();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].query, "how do I read a file?");
    assert_eq!(reqs[0].documents.len(), 3);
    assert_eq!(reqs[0].top_k, Some(3));
}

#[test]
fn timeout_option_threads_into_the_request() {
    // The per-call :timeout (ms) must reach the canonical ChatRequest the provider sees.
    let fake = FakeProvider::builder("fake").model("m").reply("ok").build();
    let (result, recorder) =
        eval_with_fake(r#"(llm/complete "hi" {:model "m" :timeout 7500})"#, fake);
    result.expect("complete should succeed");
    let reqs = recorder.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].timeout_ms, Some(7500));
}

#[test]
fn no_timeout_option_leaves_request_default() {
    let fake = FakeProvider::builder("fake").model("m").reply("ok").build();
    let (result, recorder) = eval_with_fake(r#"(llm/complete "hi" {:model "m"})"#, fake);
    result.expect("complete should succeed");
    assert_eq!(recorder.requests()[0].timeout_ms, None);
}

/// SPIKE (4.3): what does `llm/stream` actually do when the provider fails MID-stream,
/// after some chunks were already delivered to the callback? Established empirically:
/// the partial chunks ARE delivered, the error IS surfaced (catchable), and there is
/// NO auto-retry (one provider call). This is why mid-stream retry can't transparently
/// "solve" streaming failure — a retry would re-deliver the already-emitted chunks.
#[test]
fn spike_mid_stream_failure_behaviour() {
    use sema_llm::types::LlmError;
    let fake = FakeProvider::builder("fake")
        .model("m")
        .stream_then_error(
            &["Hello ", "wor"],
            LlmError::Http("connection reset by peer".to_string()),
        )
        .build();

    let src = r#"
        (define received "")
        (define errored #f)
        (try
          (llm/stream "p" (lambda (c) (set! received (string-append received c))) {:model "m"})
          (catch e (set! errored #t)))
        (list received errored)
    "#;
    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("the try/catch should yield a value, not propagate");
    let items = val.as_seq().expect("returns (received errored)");

    // (1) The partial chunks emitted BEFORE the failure were delivered to the callback.
    assert_eq!(
        items[0].as_str(),
        Some("Hello wor"),
        "partial chunks must reach the callback before the mid-stream error"
    );
    // (2) The mid-stream failure surfaced as a catchable error.
    assert_eq!(items[1], Value::bool(true), "the error must surface");
    // (3) No automatic retry — exactly one provider call (a retry would duplicate output).
    assert_eq!(
        recorder.requests().len(),
        1,
        "no auto-retry on a mid-stream failure (a retry would re-emit the partial)"
    );
}

#[test]
fn stream_budget_pre_gate_blocks_at_open() {
    // :on-stream :pre-gate makes llm/stream refuse to open once the budget is spent.
    let fake = FakeProvider::builder("fake")
        .model("costly")
        .reply_with_usage("spent", 1000, 1000) // first complete spends $2
        .stream(&["must ", "not ", "run"])
        .build();
    let src = r#"
        (llm/set-pricing "costly" 1000.0 1000.0)   ; $2 at 1000 in + 1000 out
        (define out "")
        (llm/with-budget {:max-cost-usd 0.5 :on-stream :pre-gate}
          (fn ()
            (try (llm/complete "spend" {:model "costly"}) (catch e nil))
            (try (llm/stream "p" (lambda (c) (set! out (string-append out c))) {:model "costly"})
                 (catch e (set! out "BLOCKED")))))
        out
    "#;
    let (result, recorder) = eval_with_fake(src, fake);
    assert_eq!(result.expect("eval ok").as_str(), Some("BLOCKED"));
    assert_eq!(
        recorder.requests().len(),
        1,
        "blocked stream must not reach the provider"
    );
}

#[test]
fn stream_default_does_not_pre_gate_budget() {
    // Without :on-stream :pre-gate, an over-budget stream still OPENS and emits chunks
    // (the callback fires); only the post-call usage check enforces the cap afterward.
    // Pre-gate's difference is that the callback never fires (blocked at open).
    let fake = FakeProvider::builder("fake")
        .model("costly")
        .reply_with_usage("spent", 1000, 1000)
        .stream(&["ran"])
        .build();
    let src = r#"
        (llm/set-pricing "costly" 1000.0 1000.0)
        (define got-chunk #f)
        (llm/with-budget {:max-cost-usd 0.5}
          (fn ()
            (try (llm/complete "spend" {:model "costly"}) (catch e nil))
            (try (llm/stream "p" (lambda (c) (set! got-chunk #t)) {:model "costly"})
                 (catch e nil))))
        got-chunk
    "#;
    let (result, _r) = eval_with_fake(src, fake);
    assert_eq!(
        result.expect("eval ok"),
        Value::bool(true),
        "default: stream opens and the callback fires even when over budget (no pre-gate)"
    );
}

fn eval_with_two(
    src: &str,
    p1: FakeProvider,
    p2: FakeProvider,
) -> Result<Value, sema_core::SemaError> {
    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(p1));
    register_test_provider(Box::new(p2));
    interp.eval_str_compiled(src)
}

#[test]
fn stream_fails_over_at_open() {
    // p1 errors before any chunk → fall over to p2, which streams.
    use sema_llm::types::LlmError;
    let p1 = FakeProvider::builder("p1")
        .model("m")
        .error(LlmError::Http("p1 is down".to_string()))
        .build();
    let p2 = FakeProvider::builder("p2")
        .model("m")
        .stream(&["hello ", "world"])
        .build();
    let src = r#"
        (define out "")
        (llm/with-fallback [:p1 :p2]
          (fn () (llm/stream "p" (lambda (c) (set! out (string-append out c))))))
        out
    "#;
    let val = eval_with_two(src, p1, p2).expect("fallback stream should succeed");
    assert_eq!(
        val.as_str(),
        Some("hello world"),
        "must fail over to p2 at open"
    );
}

#[test]
fn stream_does_not_fail_over_mid_stream() {
    // p1 emits a partial chunk THEN errors → surface, do NOT fall over (would duplicate).
    use sema_llm::types::LlmError;
    let p1 = FakeProvider::builder("p1")
        .model("m")
        .stream_then_error(
            &["partial"],
            LlmError::Http("dropped mid-stream".to_string()),
        )
        .build();
    let p2 = FakeProvider::builder("p2")
        .model("m")
        .stream(&["FULL"])
        .build();
    let src = r#"
        (define out "")
        (define errored #f)
        (llm/with-fallback [:p1 :p2]
          (fn ()
            (try (llm/stream "p" (lambda (c) (set! out (string-append out c))))
                 (catch e (set! errored #t)))))
        (list out errored)
    "#;
    let val = eval_with_two(src, p1, p2).expect("eval ok");
    let items = val.as_seq().unwrap();
    assert_eq!(
        items[0].as_str(),
        Some("partial"),
        "partial kept; p2's FULL must NOT appear"
    );
    assert_eq!(
        items[1],
        Value::bool(true),
        "mid-stream error surfaces (no silent failover)"
    );
}

// ---------------------------------------------------------------------------
// Conversation usage accounting (issue #12 correctness fix): conversation/say
// folds each turn's real usage into the conversation, and conversation/cost
// reports the billed sum.
// ---------------------------------------------------------------------------

#[test]
fn conversation_say_accumulates_real_usage_and_cost() {
    // Two turns, explicit token usage; a custom price for the fake's model so cost
    // is a known figure: per turn 100*$1/M + 20*$2/M = 0.00014, doubled = 0.00028.
    let fake = FakeProvider::builder("fake-priced")
        .model("fake-priced")
        .reply_with_usage("first", 100, 20)
        .reply_with_usage("second", 100, 20)
        .build();
    let src = r#"
        (llm/set-pricing "fake-priced" 1.0 2.0)
        (let* ((c0 (conversation/new {:model "fake-priced"}))
               (c1 (conversation/say c0 "hi"))
               (c2 (conversation/say c1 "again"))
               (stats (conversation/stats c2)))
          (and (= (:prompt (:tokens stats)) 200)
               (= (:completion (:tokens stats)) 40)
               (= (:total (:tokens stats)) 240)
               (> (conversation/cost c2) 0.00027)
               (< (conversation/cost c2) 0.00029)))"#;
    let (result, _rec) = eval_with_fake(src, fake);
    let val = result.expect("conversation/say should accumulate usage");
    assert_eq!(
        val,
        Value::bool(true),
        "tokens accumulate across turns and cost equals the billed sum"
    );
}

#[test]
fn conversation_cost_is_nil_without_known_pricing() {
    // A model with no price: usage still accumulates (tokens), but cost stays nil —
    // conversation/cost must NOT fall back to estimation.
    let fake = FakeProvider::builder("unpriced")
        .model("unpriced-model")
        .reply_with_usage("hello", 10, 5)
        .build();
    let src = r#"
        (let* ((c0 (conversation/new {:model "unpriced-model"}))
               (c1 (conversation/say c0 "hi")))
          (and (nil? (conversation/cost c1))
               (= (:total (:tokens (conversation/stats c1))) 15)))"#;
    let (result, _rec) = eval_with_fake(src, fake);
    let val = result.expect("conversation/say should still run without pricing");
    assert_eq!(
        val,
        Value::bool(true),
        "cost is nil (no estimation) while tokens still accumulate"
    );
}

// ── agent/run :on-text streaming (the Sema Coder TUI needs live token deltas) ──

/// `agent/run` with `:on-text` streams the assistant reply as deltas, in order,
/// and the final `:response` equals their concatenation.
#[test]
fn agent_run_on_text_streams_deltas_in_order() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .stream(&["Hel", "lo, ", "world"])
        .build();

    let src = r#"
        (defagent bot {:model "fake-model"})
        (define trace "")
        (let ((r (agent/run bot "hi"
                   {:on-text (lambda (c) (set! trace (string-append trace c "|")))})))
          (list (:response r) trace))
    "#;
    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("agent/run with :on-text should complete");
    let items = val.as_seq().expect("list result");
    assert_eq!(items[0].as_str(), Some("Hello, world"), "final response");
    assert_eq!(
        items[1].as_str(),
        Some("Hel|lo, |world|"),
        "deltas must arrive in order, as separate chunks"
    );
    assert_eq!(recorder.call_count(), 1);
}

/// Streaming must survive a tool round: round 1 issues a tool call (no visible
/// text), round 2 streams the final answer. Tool-result correlation is unchanged
/// — the second request carries the tool result — and only round 2's text streams.
#[test]
fn agent_run_on_text_streams_after_a_tool_round() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_1", "calc", serde_json::json!({"x": 2}))
        .stream(&["The ", "answer ", "is 4"])
        .build();

    let src = r#"
        (deftool calc "double a number" {:x {:type :number :description "n"}}
          (lambda (x) "4"))
        (defagent bot {:model "fake-model" :tools [calc] :max-turns 5})
        (define trace "")
        (let ((r (agent/run bot "double 2"
                   {:on-text (lambda (c)
                               (when (> (string/length c) 0)
                                 (set! trace (string-append trace c "|"))))})))
          (list (:response r) trace))
    "#;
    let (result, recorder) = eval_with_fake(src, fake);
    let val = result.expect("agent/run streaming through a tool round should complete");
    let items = val.as_seq().expect("list result");
    assert_eq!(items[0].as_str(), Some("The answer is 4"), "final response");
    assert_eq!(
        items[1].as_str(),
        Some("The |answer |is 4|"),
        "only round 2's text streams, in order"
    );
    assert_eq!(
        recorder.call_count(),
        2,
        "two provider calls: tool round + reply"
    );
}

/// Regression: multi-turn tool history must round-trip. Turn 1 calls a tool; we
/// feed its `:messages` back into turn 2. The re-sent history has to keep the
/// assistant `tool_calls` turn AND the tool-result's `tool_call_id`, or providers
/// reject it (e.g. Anthropic 400: `tool_use_id` empty). This once broke every
/// multi-turn agent conversation that used a tool.
#[test]
fn agent_run_preserves_tool_correlation_across_turns() {
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .tool_call("call_abc", "calc", serde_json::json!({"x": 2})) // turn 1, round 1
        .reply("four") // turn 1, round 2 (final)
        .reply("done") // turn 2, round 1 (final)
        .build();

    let src = r#"
        (deftool calc "double a number" {:x {:type :number :description "n"}}
          (lambda (x) "4"))
        (defagent bot {:model "fake-model" :tools [calc] :max-turns 5})
        (define hist (:messages (agent/run bot "double 2" {:messages '()})))
        (:response (agent/run bot "and again" {:messages hist}))
    "#;
    let (result, recorder) = eval_with_fake(src, fake);
    result.expect("two turns with a tool round should complete");

    // Turn 2's request is the last one recorded; it carries turn 1's history.
    let reqs = recorder.requests();
    let last = reqs.last().expect("a turn-2 request");
    assert!(
        last.messages
            .iter()
            .any(|m| m.role == "assistant" && !m.tool_calls.is_empty()),
        "re-sent history must keep the assistant tool_calls turn"
    );
    let tool_msg = last
        .messages
        .iter()
        .find(|m| m.role == "tool")
        .expect("re-sent history must contain the tool-result message");
    assert_eq!(
        tool_msg.tool_call_id.as_deref(),
        Some("call_abc"),
        "the re-sent tool result must keep its tool_call_id"
    );
    assert_eq!(
        tool_msg.tool_name.as_deref(),
        Some("calc"),
        "the re-sent tool result must keep its tool name"
    );
}
