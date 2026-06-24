//! Sema-level dynamic-workflow surface: the builtins backing the workflow DSL.
//!
//! Thin wrappers over `sema_workflow` that dispatch Sema thunks back into the SAME VM
//! via `crate::list::call_function` (the `otel/with-session` idiom):
//!
//! - `(workflow/run name doc meta thunk)` — opens a `WorkflowCtx` (journal sink under
//!   `./.sema/runs/<run-id>/`), emits `run.started`, runs the body thunk, closes the
//!   open phase, emits `run.ended`, writes `result.json`, and returns the
//!   discriminated-union `{:status …}` envelope (forced to `:failed` if a budget cap
//!   tripped).
//! - `(workflow/phase label)` — a MARKER: closes the previously-open phase and opens
//!   `label` (the checkpoints/agents that follow attribute to it).
//! - `(workflow/agent opts thunk)` — runs a leaf as a journaled agent (started/result
//!   + per-agent budget), with the resume short-circuit and budget latch at its entry.
//! - `(workflow/tool-call name [args])` — journals a tool call by the current agent.
//! - `(checkpoint :k v)` records+returns `v` (emitting a `checkpoint` event);
//!   `(checkpoint :k)` reads it back.
//!
//! The macros `defworkflow`/`phase`/`agent` (prelude) expand to these — see
//! `crates/sema-eval/src/prelude.rs`.

use sema_core::{SemaError, Value};
use sema_workflow::context;
use sema_workflow::event::WorkflowEvent;
use std::collections::BTreeMap;
use std::time::Instant;

/// A keyword/string argument as a plain `String` (checkpoint keys, phase labels).
fn as_name(v: &Value) -> Option<String> {
    v.as_keyword().or_else(|| v.as_str().map(|s| s.to_string()))
}

/// Read a string-valued option from an opts map (empty string if absent/not a map).
/// Used for the hidden `:__prompt` / `:__schema-repr` content-key inputs the `agent`
/// macro injects.
fn opt_str(v: &Value, key: &str) -> String {
    v.as_map_rc()
        .and_then(|m| {
            m.get(&Value::keyword(key))
                .and_then(|x| x.as_str().map(String::from))
        })
        .unwrap_or_default()
}

/// The workflow's declared phase plan from `defworkflow` meta `:phases` (a list or
/// vector of names — keyword OR string items, via `as_name`). Empty when absent. Lets
/// the dashboard show ALL phases up front instead of only those that have started.
fn declared_phases(meta: &Value) -> Vec<String> {
    let Some(m) = meta.as_map_rc() else {
        return Vec::new();
    };
    let Some(v) = m.get(&Value::keyword("phases")) else {
        return Vec::new();
    };
    v.as_seq()
        .map(|items| items.iter().filter_map(as_name).collect())
        .unwrap_or_default()
}

/// Resolve an agent's role label from the `workflow/agent` first argument: the `:name`
/// of an opts map, or a bare keyword/string label, falling back to "agent".
fn agent_role(v: &Value) -> String {
    if let Some(m) = v.as_map_rc() {
        if let Some(name) = m.get(&Value::keyword("name")).and_then(as_name) {
            return name;
        }
        return "agent".to_string();
    }
    as_name(v).unwrap_or_else(|| "agent".to_string())
}

/// Render a value for the journal so the dashboard can show the real data, capped
/// (char-boundary safe) so one huge value can't bloat a journal line.
fn capped_render(v: &Value) -> String {
    const MAX: usize = 4000;
    let s = sema_core::pretty_print(v, 100);
    if s.chars().count() > MAX {
        let head: String = s.chars().take(MAX).collect();
        format!("{head}\n… (truncated, {} chars total)", s.chars().count())
    } else {
        s
    }
}

/// Build the success envelope returned by `workflow/run`. PASS-THROUGH: if the
/// workflow body's last value is already a `{:status …}` map (the idiomatic shape —
/// e.g. `{:status :success :files … :findings …}`), it is returned verbatim so its
/// fields land at the top level of `result.json`. Otherwise the value is wrapped in
/// `{:status :success :value <v>}`.
fn success_envelope(value: Value) -> Value {
    if let Some(m) = value.as_map_rc() {
        if m.keys()
            .any(|k| k.as_keyword().as_deref() == Some("status"))
        {
            return value;
        }
    }
    let mut m = BTreeMap::new();
    m.insert(Value::keyword("status"), Value::keyword("success"));
    m.insert(Value::keyword("value"), value);
    Value::map(m)
}

/// Close the currently-open marker phase, if any, emitting its `phase.ended` with the
/// given status. No-op when no phase is open (a workflow with no `(phase …)` markers,
/// or after the last phase already closed). Called both by the `(phase …)` marker (to
/// close the prior phase) and by `workflow/run` at the run end (to close the last one).
fn close_open_phase(ctx: &sema_workflow::context::WorkflowCtx, status: &str) {
    if let Some((_seq, label)) = ctx.take_open_phase() {
        ctx.emit(WorkflowEvent::PhaseEnded {
            seq: ctx.next_seq(),
            ts: ctx.ts(),
            phase: label,
            status: status.into(),
            dur_ms: ctx.dur_ms(), // 0 under the fixed-ts seam
        });
    }
}

fn failed_envelope(msg: &str) -> Value {
    let mut m = BTreeMap::new();
    m.insert(Value::keyword("status"), Value::keyword("failed"));
    m.insert(Value::keyword("error"), Value::string(msg));
    Value::map(m)
}

/// The envelope for a run that a budget cap stopped. Distinct `:reason` (not `:error`)
/// because the body itself did not error — the runtime aborted it.
fn budget_failed_envelope() -> Value {
    let mut m = BTreeMap::new();
    m.insert(Value::keyword("status"), Value::keyword("failed"));
    m.insert(Value::keyword("reason"), Value::string("budget exceeded"));
    Value::map(m)
}

pub fn register(env: &sema_core::Env) {
    // (workflow/run name doc meta thunk) — open a run scope, journal start/end, return
    // the {:status ...} envelope. `name`/`doc` are strings; `meta` is the workflow's
    // metadata map ({:args ... :budget ... :perms ...}); `thunk` is the (lambda () ...)
    // wrapping the workflow body.
    crate::register_fn(env, "workflow/run", |args| {
        if args.len() != 4 {
            return Err(SemaError::arity("workflow/run", "4", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        // doc (args[1]) and meta (args[2]) are carried for the journal/metadata.json but
        // recorded into the journal/metadata.json; tolerate any shape.
        let doc = args[1].as_str().unwrap_or("").to_string();
        let meta = args[2].clone();
        let thunk = &args[3];

        // Open the run scope: sets up the journal sink under ./.sema/runs/<run-id>/,
        // installs the thread-local WorkflowCtx, and returns a panic-safe Drop guard
        // that reaps the previous scope. `set_workflow_scope` reads the
        // SEMA_WORKFLOW_RUN_ID / SEMA_WORKFLOW_FIXED_TS test seam internally.
        let guard = context::set_workflow_scope(&name, &doc, &meta)
            .map_err(|e| SemaError::eval(format!("workflow/run: {e}")))?;

        // run.started — emitted inside the live scope so seq starts at 0.
        {
            let ctx = context::current()
                .ok_or_else(|| SemaError::eval("workflow/run: scope not established"))?;
            ctx.emit(WorkflowEvent::RunStarted {
                seq: ctx.next_seq(),
                ts: ctx.ts(),
                workflow: name.clone(),
                run_id: ctx.run_id(),
                code_version: String::new(),
                args_json: ctx.args_json().to_string(),
                phases: declared_phases(&meta),
            });
        }

        // Run the body thunk in the same VM.
        let result = crate::list::call_function(thunk, &[]);

        // Derive the envelope + status, then journal run.ended and write result.json
        // BEFORE the guard drops (so the sink is still open).
        let (mut status, mut envelope, mut reason) = match &result {
            Ok(v) => ("success", success_envelope(v.clone()), None),
            Err(e) => (
                "failed",
                failed_envelope(&e.to_string()),
                Some("workflow body returned an error".to_string()),
            ),
        };

        if let Some(ctx) = context::current() {
            // A tripped budget cap fails the run regardless of the body's own outcome
            // (the latch, not an Err, is the source of truth — see workflow/agent).
            if ctx.over_budget() {
                status = "failed";
                envelope = budget_failed_envelope();
                reason = Some("budget exceeded".to_string());
            }
            // Close the last open marker phase before run.ended (its status mirrors the
            // run: a phase still open when the body errored is itself "failed").
            close_open_phase(&ctx, status);
            ctx.emit(WorkflowEvent::RunEnded {
                seq: ctx.next_seq(),
                ts: ctx.ts(),
                status: status.into(),
                reason,
                dur_ms: ctx.dur_ms(),
            });
            // result.json — the final envelope. Lossy/best-effort; swallow write errors
            // the same way the journal writer does.
            ctx.write_result(&envelope);
        }

        drop(guard);
        Ok(envelope)
    });

    // (workflow/phase label) — a MARKER (workflow.js semantics), not a wrapper. Closes
    // the previously-open phase (emitting its phase.ended) then opens `label`. The
    // checkpoints/agents that follow attribute to this phase until the next marker or
    // the run end (`workflow/run` closes the last open phase). Returns nil.
    crate::register_fn(env, "workflow/phase", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("workflow/phase", "1", args.len()));
        }
        let label = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let ctx = context::current()
            .ok_or_else(|| SemaError::eval("workflow/phase outside a workflow/run"))?;

        // Reaching a new marker means the prior phase completed successfully.
        close_open_phase(&ctx, "success");

        // The phase.started seq IS the phase's start_seq — the key checkpoints and
        // agents attribute to (so the dashboard nests them under this phase).
        let phase_seq = ctx.next_seq();
        ctx.open_phase(phase_seq, label.clone());
        ctx.emit(WorkflowEvent::PhaseStarted {
            seq: phase_seq,
            ts: ctx.ts(),
            phase: label,
        });
        Ok(Value::nil())
    });

    // (workflow/agent label thunk) — run a leaf (typically an LLM/tool call) as a
    // journaled "agent": emits agent.started before and agent.result after, so the
    // dashboard renders it as an agent row under the current phase. Returns the
    // thunk's value (or propagates its error, after journaling a result). A no-op
    // wrapper (just runs the thunk) when called outside a workflow run.
    crate::register_fn(env, "workflow/agent", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("workflow/agent", "2", args.len()));
        }
        // First arg is the agent role: an opts map `{:name "scout" …}` (the `agent`
        // macro form), or a bare keyword/string label. Default role is "agent".
        let label = agent_role(&args[0]);
        let thunk = &args[1];
        let Some(ctx) = context::current() else {
            // Outside a run: transparent — just call the thunk.
            return crate::list::call_function(thunk, &[]);
        };
        // Resume short-circuit FIRST (before the budget latch): compute this leaf's
        // content-key from its inputs (the `agent` macro injects :__prompt and
        // :__schema-repr). On a resumed run a memoized leaf replays for FREE — return it
        // WITHOUT running the model or emitting events. This MUST precede the budget
        // check: a replay makes no provider call, so a tripped cap must not refuse it
        // (refusing would return nil for a value that's on disk). The key is computed on
        // EVERY leaf so its occurrence ordinal advances in body order either way.
        let content_key = ctx.agent_content_key(
            &opt_str(&args[0], "__prompt"),
            &opt_str(&args[0], "__schema-repr"),
            &label,
            &ctx.cur_phase_label(),
        );
        if ctx.resuming() {
            if let Some(v) = ctx.memo_lookup(&content_key) {
                return Ok(v);
            }
        }
        // Budget latch: once a cap is tripped, refuse to LAUNCH further (non-replayed)
        // leaves. No events are emitted for a skipped agent — the journal shows only the
        // leaves that actually ran (the run is forced to :failed by workflow/run).
        // Checking at ENTRY (not via Err) is what makes the cap hold through
        // `__fanout-tagged`, which would otherwise swallow a leaf Err into nil.
        if ctx.over_budget() {
            return Ok(Value::nil());
        }
        // Unique per-invocation id (the dashboard correlates started→result→budget
        // by it); the label is the agent_name (role).
        let agent_id = ctx.next_agent_id(&label);
        ctx.emit(WorkflowEvent::AgentStarted {
            seq: ctx.next_seq(),
            ts: ctx.ts(),
            agent_id: agent_id.clone(),
            agent_name: label.clone(),
            model: String::new(), // unknown until the call completes (filled below)
        });
        let start = Instant::now();
        // Clear the per-thread usage slot BEFORE the thunk so the snapshot afterwards
        // reflects ONLY a completion this leaf made. Without this, a leaf whose call
        // fails (or makes none) would re-read the PREVIOUS leaf's sticky usage — a
        // phantom budget event + a double-charge against the cap.
        sema_llm::builtins::clear_last_usage();
        // Mark this as the current agent so `workflow/tool-call` inside the thunk
        // attributes to it; clear afterwards. NOTE: `cur_agent` is a single shared slot,
        // so under a CONCURRENT `:tools` fan-out (tool loops yield between rounds) two
        // in-flight tool-agents can clobber each other's attribution — tool-call
        // attribution is best-effort under fan-out, the same single-slot-thread-local
        // root as the per-agent budget caveat. The real fix is per-task scoping in the
        // scheduler (out of scope); a sequential body is exact.
        ctx.set_cur_agent(Some(agent_id.clone()));
        let result = crate::list::call_function(thunk, &[]);
        ctx.set_cur_agent(None);
        let dur_ms = if ctx.deterministic() {
            0
        } else {
            start.elapsed().as_millis() as u64
        };
        // Per-agent usage: the most recent LLM completion on this thread (if the leaf
        // made one) gives the model + tokens + cost to attribute to this agent.
        let usage = sema_llm::builtins::last_usage_snapshot();
        let model = usage.as_ref().map(|u| u.model.clone()).unwrap_or_default();
        // The agent's actual output, captured in the journal so the dashboard can
        // show it (not a hash). Length-capped to bound the journal line.
        let output = match &result {
            Ok(v) => capped_render(v),
            Err(e) => format!("error: {e}"),
        };
        let status = if result.is_ok() { "ok" } else { "failed" };
        ctx.emit(WorkflowEvent::AgentResult {
            seq: ctx.next_seq(),
            ts: ctx.ts(),
            agent_id: agent_id.clone(),
            status: status.into(),
            output,
            dur_ms,
            model,
        });
        // Attribute token usage + cost to this agent via a budget event (only when a
        // completion actually ran — otherwise per-agent tokens stay honestly absent).
        if let Some(u) = usage {
            ctx.emit(WorkflowEvent::Budget {
                seq: ctx.next_seq(),
                ts: ctx.ts(),
                agent_id: Some(agent_id),
                phase_seq: ctx.phase_seq(),
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cost_usd: u.cost_usd,
                budget_limit: ctx.budget_limit_for_event(),
            });
            // Charge AFTER the Budget event is journaled, so the leaf that tips the cap
            // is itself fully recorded; the sticky latch then refuses the NEXT leaf.
            ctx.charge(u.cost_usd, u.input_tokens + u.output_tokens);
        }
        // Memoize the leaf's value for a future --resume (round-trip-guarded inside
        // memo_store; called outside any held journal borrow).
        if let Ok(ref v) = result {
            ctx.memo_store(&content_key, v);
        }
        result
    });

    // (workflow/tool-call tool-name [args]) — journal a tool call by the current
    // agent (the dashboard renders these as tool twigs in the agent's drill-in).
    // No-op (returns nil) outside a workflow/agent. `args` is an opaque/gated
    // descriptor; pass a string or omit for the "gated" sentinel.
    crate::register_fn(env, "workflow/tool-call", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("workflow/tool-call", "1-2", args.len()));
        }
        let tool_name = as_name(&args[0])
            .ok_or_else(|| SemaError::type_error("keyword or string", args[0].type_name()))?;
        if let Some(ctx) = context::current() {
            if let Some(agent_id) = ctx.cur_agent() {
                // The args descriptor is a string (the manual demo form) OR a structured
                // value (the real on-tool-call callback passes the tool's arg map) —
                // render the latter so args_json carries the real call args, not "gated".
                let args_json = match args.get(1) {
                    None => "gated".to_string(),
                    Some(v) => v
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| capped_render(v)),
                };
                ctx.emit(WorkflowEvent::AgentToolCall {
                    seq: ctx.next_seq(),
                    ts: ctx.ts(),
                    agent_id,
                    tool_name,
                    args_json,
                });
            }
        }
        Ok(Value::nil())
    });

    // (checkpoint :k v) records+returns v and emits a checkpoint event;
    // (checkpoint :k) reads the stored value (nil if unset).
    crate::register_fn(env, "checkpoint", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("checkpoint", "1-2", args.len()));
        }
        let key = as_name(&args[0])
            .ok_or_else(|| SemaError::type_error("keyword or string", args[0].type_name()))?;
        let ctx = context::current()
            .ok_or_else(|| SemaError::eval("checkpoint outside a workflow/run"))?;

        if args.len() == 2 {
            // Write: store, journal, return the value (so it threads through `let`).
            let value = args[1].clone();
            // Resume short-circuit: a memoized checkpoint returns the recorded value,
            // seeds the state bag (so later `(checkpoint :k)` reads see it), and skips
            // re-emitting the checkpoint event. The key advances its occurrence ordinal
            // on every run (computed before the resume check) to stay in body order.
            let resume_key = ctx.checkpoint_content_key(&key, &ctx.cur_phase_label());
            if ctx.resuming() {
                if let Some(v) = ctx.memo_lookup(&resume_key) {
                    ctx.store_checkpoint(&key, v.clone());
                    return Ok(v);
                }
            }
            ctx.store_checkpoint(&key, value.clone());
            let digest = ctx.value_digest(&value);
            let content_key = ctx.content_key(&key, &digest);
            ctx.emit(WorkflowEvent::Checkpoint {
                seq: ctx.next_seq(),
                ts: ctx.ts(),
                phase_seq: ctx.phase_seq(),
                key: key.clone(),
                content_key,
                value_digest: digest,
                value: capped_render(&value), // the real checkpointed value, for the dashboard
            });
            // Memoize for a future --resume (round-trip-guarded; outside journal borrow).
            ctx.memo_store(&resume_key, &value);
            Ok(value)
        } else {
            // Read: return the stored value or nil.
            Ok(ctx.read_checkpoint(&key).unwrap_or_else(Value::nil))
        }
    });
}
