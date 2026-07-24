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
//! - `(workflow/step opts thunk)` — runs a leaf as a journaled step (agent.started/
//!   result + per-step budget), with the resume short-circuit and budget latch at its entry.
//! - `(workflow/tool-call name [args])` — journals a tool call by the current agent.
//! - `(workflow/checkpoint :k thunk)` records+returns `(thunk)` (emitting a
//!   `checkpoint` event); `(workflow/checkpoint :k)` reads it back. The public
//!   `(checkpoint ...)` macro delays writes into this backend.
//!
//! The macros `defworkflow`/`phase`/`agent` (prelude) expand to these — see
//! `crates/sema-eval/src/prelude.rs`.

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    CancelDisposition, CancelHook, CancelHookError, CompletionDecoder, CompletionKind,
    DecodedCompletion, ExternalFailure, InterruptibleResource, NativeCall, NativeCallContext,
    NativeContinuation, NativeOutcome, NativeResult, NativeSuspend, PreparedExternalOperation,
    ResumeInput, SendPayload, TaskContextHandle, Trace, WaitKind,
};
use sema_core::{SemaError, Value};
use sema_workflow::context;
use sema_workflow::event::WorkflowEvent;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::workflow_mcp::{self, ServerResolution, WorkflowMcpResolver};

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

/// Resolve a step's role label from the `workflow/step` first argument: the `:name`
/// of an opts map, or a bare keyword/string label, falling back to "step".
fn agent_role(v: &Value) -> String {
    if let Some(m) = v.as_map_rc() {
        if let Some(name) = m.get(&Value::keyword("name")).and_then(as_name) {
            return name;
        }
        return "step".to_string();
    }
    as_name(v).unwrap_or_else(|| "step".to_string())
}

/// Length-cap a string for the journal (char-boundary safe) so one huge value can't
/// bloat a journal line. The total char count is appended when truncation happens.
fn cap_text(s: &str) -> String {
    const MAX: usize = 4000;
    if s.chars().count() > MAX {
        let head: String = s.chars().take(MAX).collect();
        format!("{head}\n… (truncated, {} chars total)", s.chars().count())
    } else {
        s.to_string()
    }
}

/// Max bytes of a value's compact form the journal renders inline before truncating.
/// Golden values are tiny (far below this), so [`capped_render`] returns `pretty_print`
/// verbatim for them and the goldens stay byte-identical; only a pathologically large
/// value is truncated — and it is NEVER materialized in full (the compact form is
/// bounded-checked via `context::compact_capped`, which aborts at the cap).
const RENDERED_VALUE_MAX_BYTES: usize = 8192;

/// Render a value for the journal so the dashboard can show the real data, byte-budgeted
/// so one huge value can't materialize a multi-MB string on the VM thread. A value that
/// fits renders exactly as before (`pretty_print(v, 100)`) — keeping goldens
/// byte-identical; an over-cap value is rendered from its bounded compact prefix + a
/// truncation marker.
fn capped_render(v: &Value) -> String {
    let (compact, truncated) = sema_workflow::context::compact_capped(v, RENDERED_VALUE_MAX_BYTES);
    if truncated {
        format!("{compact}\n… (truncated at {RENDERED_VALUE_MAX_BYTES} bytes)")
    } else {
        sema_core::pretty_print(v, 100)
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

/// The `{:status :needs-auth …}` envelope (docs/plans/2026-06-24-workflow-mcp-auth.md
/// §3): `:servers` is the plain list of alias strings (plan-exact shape — a `sema
/// mcp login <alias>` / dashboard consumer keys off it); `:auth` is the additive
/// detail vector (server/url/persist) a login-guidance UI uses. `needs_auth` is
/// `(alias, url, persist)` triples in resolution order.
fn needs_auth_envelope(needs_auth: &[(String, String, String)]) -> Value {
    let mut m = BTreeMap::new();
    m.insert(Value::keyword("status"), Value::keyword("needs-auth"));
    m.insert(
        Value::keyword("servers"),
        Value::list(
            needs_auth
                .iter()
                .map(|(alias, _url, _persist)| Value::string(alias))
                .collect(),
        ),
    );
    m.insert(
        Value::keyword("auth"),
        Value::list(
            needs_auth
                .iter()
                .map(|(alias, url, persist)| {
                    let mut e = BTreeMap::new();
                    e.insert(Value::keyword("server"), Value::string(alias));
                    e.insert(Value::keyword("url"), Value::string(url));
                    e.insert(Value::keyword("persist"), Value::string(persist));
                    Value::map(e)
                })
                .collect(),
        ),
    );
    Value::map(m)
}

/// Close a run BEFORE the body thunk ever ran: close the last open phase (a
/// no-op — the implicit auth-resolution step runs before any `(phase …)`
/// marker), emit `run.ended`, write `result.json`, drop the scope guard.
/// Shared by every pre-body `:mcp` gate exit (an invalid declaration, no
/// resolver registered, or the resolver reporting `Failed`/`NeedsAuth`) — on
/// all of these the workflow body NEVER runs.
fn end_run_before_body(
    ctx: &context::WorkflowCtx,
    guard: context::WorkflowGuard,
    status: &str,
    reason: String,
    envelope: Value,
) -> (Value, Receiver<()>) {
    close_open_phase(ctx, status);
    ctx.emit(WorkflowEvent::RunEnded {
        seq: ctx.next_seq(),
        ts: ctx.ts(),
        status: status.into(),
        reason: Some(reason),
        dur_ms: ctx.dur_ms(),
    });
    ctx.write_result(&envelope);
    // Enqueue the terminal flush barrier BEFORE removing the scope, so the pre-body gate
    // exits are journal-durable exactly like a body that ran (see `terminal_plan`).
    let ack = ctx.request_flush();
    drop(guard);
    (envelope, ack)
}

/// Post-thunk teardown for a `register_thunk_fn` native: given the owning task context
/// (so the live scope resolves off the task, not TLS), the teardown state, the thunk's
/// result, and whether this is a DURABLE terminal (a normal return / body error, vs. a
/// cancellation), journal/close and produce the builtin's outcome. Returns a
/// `NativeOutcome` so `workflow/run`'s terminal can PARK on the External journal
/// flush-ack (durable normal completion) rather than only returning a value.
type FinishFn<T> =
    fn(Option<&TaskContextHandle>, T, Result<Value, SemaError>, bool) -> NativeResult;
/// Trace the `Value` edges a teardown state carries (a run's open MCP handles; none for
/// a step).
type TraceTeardownFn<T> = fn(&T, &mut dyn FnMut(GcEdge<'_>));

/// Register a thunk-taking workflow builtin (`workflow/run`, `workflow/step`) as a
/// DUAL-ABI native. `plan` runs the synchronous PRE-thunk work (scope setup, budget
/// gate, resume short-circuit) and decides whether a thunk needs to run. Under a
/// runtime quantum the runtime callback drives that thunk as a cooperative
/// `NativeOutcome::Call`, so an async op inside it (a `parallel`/`pipeline` fan-out's
/// `async/spawn`, an offloaded `llm/chat` tool loop, `channel/*`) parks on the active
/// task instead of hitting the runtime-only error stub a synchronous `call_function`
/// re-entry would. Everywhere else the legacy callback runs the thunk inline. The
/// post-thunk teardown (journaling the result, budget accounting, memoization, closing
/// the scope) is `finish`, run identically by the legacy path and the continuation.
fn register_thunk_fn<T: 'static>(
    env: &sema_core::Env,
    name: &'static str,
    plan: impl Fn(Option<&TaskContextHandle>, &[Value]) -> Result<ThunkPlan<T>, SemaError> + 'static,
    finish: FinishFn<T>,
    trace_teardown: TraceTeardownFn<T>,
) {
    let plan = Rc::new(plan);
    let for_legacy = plan.clone();
    let for_runtime = plan;
    env.set(
        sema_core::intern(name),
        Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            name,
            // Host arm (outside a runtime quantum): no task context — scope resolves off
            // the `WORKFLOW` thread-local fallback.
            move |args| match for_legacy(None, args)? {
                ThunkPlan::Immediate(value) => Ok(value),
                ThunkPlan::Run { thunk, teardown } => {
                    let result = crate::list::call_function(&thunk, &[]);
                    // Host arm: no External wait to park on, so a durable terminal flush is a
                    // bounded blocking wait inside `finish`; the outcome is always a value.
                    match finish(None, teardown, result, true)? {
                        NativeOutcome::Return(value) => Ok(value),
                        _ => Err(SemaError::eval(format!(
                            "{name}: host teardown must resolve to a value"
                        ))),
                    }
                }
                // The host arm resolves `:mcp` synchronously (its `io_block_on` is legal
                // off the quantum), so it never asks for a suspend and cannot drive one.
                ThunkPlan::Suspend(_) => Err(SemaError::eval(format!(
                    "{name}: cannot offload MCP resolution outside the runtime"
                ))),
            },
            // Runtime arm: install/read/remove the scope on the OWNING TASK's context, so
            // a sibling task interleaved on the thread never resolves to this run.
            move |ctx, args| {
                let task_context = ctx.task_context.clone();
                match for_runtime(Some(&task_context), args)? {
                    ThunkPlan::Immediate(value) => Ok(NativeOutcome::Return(value)),
                    ThunkPlan::Run { thunk, teardown } => Ok(NativeOutcome::Call(NativeCall {
                        callable: thunk,
                        args: Vec::new(),
                        continuation: Box::new(ThunkContinuation {
                            teardown: Some(teardown),
                            finish,
                            trace_teardown,
                            name,
                        }),
                    })),
                    ThunkPlan::Suspend(suspend) => Ok(NativeOutcome::Suspend(suspend)),
                }
            },
        )),
    );
}

/// Register a non-thunk workflow builtin (`workflow/phase`, `workflow/tool-call`,
/// `workflow/mcp-handle`) as a DUAL-ABI native so its runtime arm reads the task-scoped
/// workflow state instead of the `WORKFLOW` thread-local. The host arm passes `None`
/// (host fallback); the runtime arm passes the owning task's context.
fn register_scoped_fn(
    env: &sema_core::Env,
    name: &'static str,
    body: impl Fn(Option<&TaskContextHandle>, &[Value]) -> Result<Value, SemaError> + 'static,
) {
    let body = Rc::new(body);
    let for_legacy = body.clone();
    let for_runtime = body;
    env.set(
        sema_core::intern(name),
        Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            name,
            move |args| for_legacy(None, args),
            move |ctx, args| {
                let task_context = ctx.task_context.clone();
                Ok(NativeOutcome::Return(for_runtime(
                    Some(&task_context),
                    args,
                )?))
            },
        )),
    );
}

/// A workflow builtin's pre-thunk decision: either an immediate value (nothing to run —
/// a resume replay, a budget-tripped skip, or a pre-body `:mcp` gate exit), a thunk to
/// run with the `teardown` state its `finish` needs afterward, or — only under a runtime
/// quantum, only `workflow/run` — a structural suspend that offloads the blocking `:mcp`
/// resolve off the VM thread and resumes in a continuation. The host arm never produces
/// `Suspend` (it resolves synchronously), so it rejects one it can't drive.
enum ThunkPlan<T> {
    Immediate(Value),
    Run { thunk: Value, teardown: T },
    Suspend(NativeSuspend),
}

/// Cooperative teardown for a `register_thunk_fn` native: the runtime drives the thunk
/// and resumes here with its result; `finish` runs the same post-thunk work the legacy
/// synchronous path runs. Any `Value` the teardown state carries (e.g. a run's open MCP
/// handles) is traced via `trace_teardown`.
struct ThunkContinuation<T> {
    teardown: Option<T>,
    finish: FinishFn<T>,
    trace_teardown: TraceTeardownFn<T>,
    name: &'static str,
}

impl<T> Trace for ThunkContinuation<T> {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        if let Some(teardown) = &self.teardown {
            (self.trace_teardown)(teardown, sink);
        }
        true
    }
}

impl<T: 'static> NativeContinuation for ThunkContinuation<T> {
    fn resume(
        mut self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        // `durable` gates the terminal journal flush-ack: a normal return or a body error
        // parks on the flush barrier (the journal must be on disk when `workflow/run`
        // returns); a CANCELLATION skips it (the task is being torn down and must settle
        // promptly without joining the writer).
        let (result, durable) = match input {
            ResumeInput::Returned(value) => (Ok(value), true),
            ResumeInput::Failed(error) => (Err(error), true),
            ResumeInput::Cancelled(reason) => (
                Err(SemaError::eval(format!(
                    "{} thunk was cancelled ({reason:?})",
                    self.name
                ))),
                false,
            ),
            ResumeInput::Runtime(_) => (
                Err(SemaError::eval(format!(
                    "{} teardown received an unexpected runtime response",
                    self.name
                ))),
                false,
            ),
        };
        let teardown = self
            .teardown
            .take()
            .expect("thunk continuation resumed once");
        let task_context = context.task_context.clone();
        (self.finish)(Some(&task_context), teardown, result, durable)
    }
}

/// Post-thunk teardown state for `workflow/step` (`None` = a run-less transparent call,
/// which just returns the thunk's result). Carries no `Value`.
struct StepTeardown {
    agent_id: String,
    content_key: String,
    start: Instant,
    usage_scope: sema_llm::builtins::UsageScope,
}

/// Journal a `workflow/step` leaf's result: emit `agent.result`, attribute usage via a
/// `budget` event + charge the run, and memoize the value for `--resume`. Re-fetches the
/// live scope (still installed by the enclosing `workflow/run` guard) rather than
/// capturing it. Shared by the legacy and cooperative paths.
fn finish_step(
    task_context: Option<&TaskContextHandle>,
    teardown: Option<StepTeardown>,
    result: Result<Value, SemaError>,
    _durable: bool,
) -> NativeResult {
    let Some(td) = teardown else {
        // Transparent (outside a run): nothing to journal.
        return result.map(NativeOutcome::Return);
    };
    let Some(ctx) = context::current_for(task_context) else {
        return result.map(NativeOutcome::Return);
    };
    context::set_cur_agent_for(task_context, None);
    let dur_ms = if ctx.deterministic() {
        0
    } else {
        td.start.elapsed().as_millis() as u64
    };
    let usage = td.usage_scope.usage();
    let model = usage.model.clone();
    let output = match &result {
        Ok(v) => capped_render(v),
        Err(e) => format!("error: {e}"),
    };
    let status = if result.is_ok() { "ok" } else { "failed" };
    ctx.emit(WorkflowEvent::AgentResult {
        seq: ctx.next_seq(),
        ts: ctx.ts(),
        agent_id: td.agent_id.clone(),
        status: status.into(),
        output,
        dur_ms,
        model,
    });
    if usage.calls > 0 {
        ctx.emit(WorkflowEvent::Budget {
            seq: ctx.next_seq(),
            ts: ctx.ts(),
            agent_id: Some(td.agent_id),
            phase_seq: ctx.phase_seq(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: usage.cost_usd,
            budget_limit: ctx.budget_limit_for_event(),
        });
        // Charge AFTER the Budget event is journaled, so the leaf that tips the cap is
        // itself fully recorded; the sticky latch then refuses the NEXT leaf.
        ctx.charge(usage.cost_usd, usage.input_tokens + usage.output_tokens);
    }
    if let Ok(ref v) = result {
        ctx.memo_store(&td.content_key, v);
    }
    result.map(NativeOutcome::Return)
}

/// Pre-thunk work for `workflow/step` — see the original inline documentation preserved
/// in `finish_step` and the event emissions below.
fn step_plan(
    task_context: Option<&TaskContextHandle>,
    args: &[Value],
) -> Result<ThunkPlan<Option<StepTeardown>>, SemaError> {
    if args.len() != 2 {
        return Err(SemaError::arity("workflow/step", "2", args.len()));
    }
    // First arg is the agent role: an opts map `{:name "scout" …}` (the `agent` macro
    // form), or a bare keyword/string label. Default role is "agent".
    let label = agent_role(&args[0]);
    let thunk = args[1].clone();
    let Some(ctx) = context::current_for(task_context) else {
        // Outside a run: transparent — just call the thunk (still cooperatively, so an
        // async op inside it works), with no journaling teardown.
        return Ok(ThunkPlan::Run {
            thunk,
            teardown: None,
        });
    };
    // Resume short-circuit FIRST (before the budget latch): a memoized leaf replays for
    // FREE. This MUST precede the budget check: a replay makes no provider call, so a
    // tripped cap must not refuse it. The key is computed on EVERY leaf so its occurrence
    // ordinal advances in body order either way.
    let prompt = {
        let injected = opt_str(&args[0], "__prompt");
        if injected.is_empty() {
            opt_str(&args[0], "prompt")
        } else {
            injected
        }
    };
    let content_key = ctx.agent_content_key(
        &prompt,
        &opt_str(&args[0], "__schema-repr"),
        &label,
        &ctx.cur_phase_label(),
    );
    if ctx.resuming() {
        if let Some(v) = ctx.memo_lookup(&content_key) {
            return Ok(ThunkPlan::Immediate(v));
        }
    }
    // Budget latch: once a cap is tripped, refuse to LAUNCH further (non-replayed) leaves.
    if ctx.over_budget() {
        return Ok(ThunkPlan::Immediate(Value::nil()));
    }
    // Unique per-invocation id (the dashboard correlates started→result→budget by it).
    let agent_id = ctx.next_agent_id(&label);
    ctx.emit(WorkflowEvent::AgentStarted {
        seq: ctx.next_seq(),
        ts: ctx.ts(),
        agent_id: agent_id.clone(),
        agent_name: label.clone(),
        model: String::new(),
        prompt: cap_text(&prompt),
    });
    let start = Instant::now();
    // Open a per-leaf usage accumulator for the duration of this thunk; the async path
    // captures the frame's Rc into its poller so a sibling leaf under parallel/pipeline
    // fan-out can't clobber the tally. The guard pops the frame when the teardown drops it.
    let usage_scope = sema_llm::builtins::open_usage_scope();
    // Mark this as the current agent so `workflow/tool-call` inside the thunk attributes
    // to it; cleared in `finish_step`. `cur_agent` is TASK-PRIVATE, so two concurrent
    // steps on sibling tasks keep distinct active agents (no cross-attribution); a child
    // spawned inside the thunk inherits this attribution.
    context::set_cur_agent_for(task_context, Some(agent_id.clone()));
    Ok(ThunkPlan::Run {
        thunk,
        teardown: Some(StepTeardown {
            agent_id,
            content_key,
            start,
            usage_scope,
        }),
    })
}

/// Scalar state needed after a checkpoint write thunk settles. The live workflow
/// context is re-fetched in `finish_checkpoint`; retaining it here would also retain
/// its state bag of GC-visible `Value`s outside the collector's trace graph.
struct CheckpointTeardown {
    key: String,
    resume_key: String,
}

fn checkpoint_plan(
    task_context: Option<&TaskContextHandle>,
    args: &[Value],
) -> Result<ThunkPlan<CheckpointTeardown>, SemaError> {
    if args.is_empty() || args.len() > 2 {
        return Err(SemaError::arity("workflow/checkpoint", "1-2", args.len()));
    }
    let key = as_name(&args[0])
        .ok_or_else(|| SemaError::type_error("keyword or string", args[0].type_name()))?;
    let ctx = context::current_for(task_context)
        .ok_or_else(|| SemaError::eval("checkpoint outside a workflow/run"))?;

    if args.len() == 1 {
        return Ok(ThunkPlan::Immediate(
            ctx.read_checkpoint(&key).unwrap_or_else(Value::nil),
        ));
    }

    // Advancing the occurrence ordinal precedes the resume lookup, so memo hits
    // and misses consume the same body-order key.
    let resume_key = ctx.checkpoint_content_key(&key, &ctx.cur_phase_label());
    if ctx.resuming() {
        if let Some(value) = ctx.memo_lookup(&resume_key) {
            ctx.store_checkpoint(&key, value.clone());
            return Ok(ThunkPlan::Immediate(value));
        }
    }

    Ok(ThunkPlan::Run {
        thunk: args[1].clone(),
        teardown: CheckpointTeardown { key, resume_key },
    })
}

fn finish_checkpoint(
    task_context: Option<&TaskContextHandle>,
    teardown: CheckpointTeardown,
    result: Result<Value, SemaError>,
    _durable: bool,
) -> NativeResult {
    let value = result?;
    let Some(ctx) = context::current_for(task_context) else {
        return Ok(NativeOutcome::Return(value));
    };
    ctx.store_checkpoint(&teardown.key, value.clone());
    let digest = ctx.value_digest(&value);
    ctx.emit(WorkflowEvent::Checkpoint {
        seq: ctx.next_seq(),
        ts: ctx.ts(),
        phase_seq: ctx.phase_seq(),
        key: teardown.key,
        content_key: teardown.resume_key.clone(),
        value_digest: digest,
        value: capped_render(&value),
    });
    ctx.memo_store(&teardown.resume_key, &value);
    Ok(NativeOutcome::Return(value))
}

/// Post-thunk teardown state for `workflow/run`. Holds the scope guard (whose Drop
/// removes the exact scope token, LAST — after `run.ended` + `result.json`) and, until
/// closed exactly once, the resolver + open MCP handles (the handles are `Value`s —
/// traced).
struct RunTeardown {
    // A pure RAII drop guard: never read by name (a type with a manual `Drop` cannot be
    // destructured), it exists solely so its own `Drop` removes the exact scope token
    // whenever the `RunTeardown` is dropped — on `finish_run`, or via the backstop.
    #[allow(dead_code)]
    guard: context::WorkflowGuard,
    mcp: Option<McpClose>,
}

/// Resolver + open MCP handles to close exactly once — whether the run ends normally,
/// errors, is cancelled, or its continuation is dropped without ever resuming.
struct McpClose {
    resolver: Rc<dyn WorkflowMcpResolver>,
    handles: Vec<Value>,
}

impl RunTeardown {
    /// Close the run's MCP handles exactly once (idempotent — the `Drop` backstop calls
    /// this too). A run with no `:mcp` has nothing to close.
    fn close_mcp(&mut self) {
        if let Some(mcp) = self.mcp.take() {
            mcp.resolver.close(&mcp.handles);
        }
    }
}

impl Drop for RunTeardown {
    fn drop(&mut self) {
        // Backstop: a continuation dropped WITHOUT resume (runtime teardown) still closes
        // MCP exactly once; the `guard` field's own Drop then removes the exact scope
        // token, so no run leaks its handles or its scope.
        self.close_mcp();
    }
}

/// Derive the run envelope from the body's result, journal `run.ended`, write
/// `result.json`, close any MCP handles, then remove the scope token. Shared by the
/// legacy and cooperative paths; always produces an envelope (a body error becomes a
/// failed one).
///
/// `durable` (a normal return or a body error, NOT a cancellation) requests the terminal
/// journal flush barrier: the runtime path PARKS on the External flush-ack so a normal
/// `workflow/run` return means `events.jsonl`/`result.json` are on disk; the host path
/// bounded-waits. A cancellation skips the barrier — the task is being torn down and must
/// settle promptly without joining the writer (the writer drains independently).
fn finish_run(
    task_context: Option<&TaskContextHandle>,
    mut teardown: RunTeardown,
    result: Result<Value, SemaError>,
    durable: bool,
) -> NativeResult {
    let (mut status, mut envelope, mut reason) = match &result {
        Ok(v) => ("success", success_envelope(v.clone()), None),
        Err(e) => (
            "failed",
            failed_envelope(&e.to_string()),
            Some("workflow body returned an error".to_string()),
        ),
    };
    // Close any resolved MCP handles exactly once, regardless of how the body exited.
    teardown.close_mcp();
    let ctx = context::current_for(task_context);
    let ack = if let Some(ctx) = &ctx {
        // A tripped budget cap fails the run regardless of the body's own outcome.
        if ctx.over_budget() {
            status = "failed";
            envelope = budget_failed_envelope();
            reason = Some("budget exceeded".to_string());
        }
        close_open_phase(ctx, status);
        ctx.emit(WorkflowEvent::RunEnded {
            seq: ctx.next_seq(),
            ts: ctx.ts(),
            status: status.into(),
            reason,
            dur_ms: ctx.dur_ms(),
        });
        ctx.write_result(&envelope);
        // Terminal durability barrier — enqueue a flush and keep its ack receiver so a
        // normal completion guarantees the journal is on disk before returning.
        durable.then(|| ctx.request_flush())
    } else {
        None
    };
    // Dropping the teardown removes the exact scope token (its `guard`) and is a no-op
    // second MCP close.
    drop(teardown);
    match ack {
        // Runtime (quantum) normal completion: park on the External flush-ack so the task
        // resumes — returning the envelope — only once the writer has flushed to disk.
        Some(ack_rx) if task_context.is_some() => Ok(NativeOutcome::Suspend(
            build_flush_ack_suspend(envelope, ack_rx),
        )),
        // Host (non-quantum) path: bounded blocking wait for the same barrier.
        Some(ack_rx) => {
            let _ = ack_rx.recv_timeout(HOST_FLUSH_ACK_TIMEOUT);
            Ok(NativeOutcome::Return(envelope))
        }
        // Cancellation (no barrier) or no live scope: return the envelope directly.
        None => Ok(NativeOutcome::Return(envelope)),
    }
}

// ── terminal journal flush barrier ─────────────────────────────────────────────
//
// A normal `workflow/run` return must mean the journal is complete on disk. Every
// terminal path (a body that ran → `finish_run`; a pre-body `:mcp` gate exit →
// `end_run_before_body`) enqueues `run.ended` + `result.json` + a `Flush` barrier, then
// the RUNTIME path parks on THIS External wait: an interruptible-blocking job that just
// awaits the writer's ack on a blocking-tier worker (no `io_block_on`, no fs on the VM
// thread — the same shape Cluster W's `resolve_prepared` uses). The decoder replays the
// run envelope the barrier carried; a cancelled park skips the ack (the writer keeps
// draining independently) and settles the task promptly without joining the writer.

/// Completion tag for the terminal journal flush barrier (`"wfls"`).
const FLUSH_ACK_COMPLETION_KIND: u64 = 0x7766_6c73;

/// Bounded wait for the host (non-quantum) terminal flush barrier.
const HOST_FLUSH_ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Build the External flush-ack suspension: a blocking-tier job awaits `ack_rx`; on ack
/// the decoder returns `envelope` and the continuation delivers it. Used by every runtime
/// terminal path so a normal return is journal-durable.
fn build_flush_ack_suspend(envelope: Value, ack_rx: Receiver<()>) -> NativeSuspend {
    let kind = CompletionKind::try_from_raw(FLUSH_ACK_COMPLETION_KIND)
        .expect("flush-ack completion kind is nonzero");
    let resource = InterruptibleResource::new("workflow/journal-flush", Box::new(FlushCancelHook));
    let prepared = PreparedExternalOperation::interruptible_blocking(
        kind,
        Box::new(FlushDecoder {
            envelope: Some(envelope),
        }),
        resource,
        move || {
            // Block on the writer's ack on a blocking-tier worker (NOT the VM thread). A
            // cancelled park never resolves this meaningfully — the ack may never come and
            // the cancel hook has already reaped.
            let _ = ack_rx.recv();
            Ok(Box::new(()) as SendPayload)
        },
    );
    NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation: Box::new(FlushContinuation),
    }
}

/// A terminal envelope whose run already ended (a pre-body gate exit), turned into a
/// durable `ThunkPlan`: the runtime arm parks on the External flush-ack; the host arm
/// bounded-waits. `ack` is the flush barrier receiver from `ctx.request_flush()`.
fn terminal_plan(
    task_context: Option<&TaskContextHandle>,
    envelope: Value,
    ack: Receiver<()>,
) -> ThunkPlan<RunTeardown> {
    if task_context.is_some() {
        ThunkPlan::Suspend(build_flush_ack_suspend(envelope, ack))
    } else {
        let _ = ack.recv_timeout(HOST_FLUSH_ACK_TIMEOUT);
        ThunkPlan::Immediate(envelope)
    }
}

/// Decoder for [`build_flush_ack_suspend`]: ignores the (unit) job payload and returns the
/// run envelope it carried on the VM thread. Holds the envelope `Value` (traced) across the
/// suspension. Not `Send`; the runtime keeps it on the VM thread.
struct FlushDecoder {
    envelope: Option<Value>,
}

impl Trace for FlushDecoder {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        if let Some(envelope) = &self.envelope {
            sink(GcEdge::Value(envelope));
        }
        true
    }
}

impl CompletionDecoder for FlushDecoder {
    fn decode(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        result.map_err(|failure| {
            SemaError::eval(format!("workflow/run journal flush: {}", failure.message()))
        })?;
        Ok(self.envelope.take().expect("flush decoder runs once"))
    }
}

/// Resumes the parked `workflow/run` with the flushed envelope. A cancellation propagates
/// (the run's result is already computed and best-effort journaled; the task is being torn
/// down), settling promptly without joining the writer.
struct FlushContinuation;

impl Trace for FlushContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for FlushContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "workflow/run journal flush was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "workflow/run: unexpected runtime response awaiting journal flush",
            )),
        }
    }
}

/// No-op cancel hook: the writer drains independently, so a cancelled flush park has
/// nothing to abort — the resource is reaped and the task settles at once.
struct FlushCancelHook;

impl Trace for FlushCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for FlushCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

/// Pre-thunk work for `workflow/run`: open the run scope, journal `run.started`, resolve
/// any declared `:mcp` servers (a pre-body gate that can end the run before the body ever
/// runs), and hand back the body thunk plus the teardown state. Mirrors the original
/// inline builtin; the post-body work moved to `finish_run`.
fn run_plan(
    task_context: Option<&TaskContextHandle>,
    args: &[Value],
) -> Result<ThunkPlan<RunTeardown>, SemaError> {
    if args.len() != 4 {
        return Err(SemaError::arity("workflow/run", "4", args.len()));
    }
    let name = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
        .to_string();
    // doc (args[1]) and meta (args[2]) are recorded into the journal/metadata.json;
    // tolerate any shape.
    let doc = args[1].as_str().unwrap_or("").to_string();
    let meta = args[2].clone();
    let thunk = args[3].clone();

    // Open the run scope: sets up the journal sink under ./.sema/runs/<run-id>/, installs
    // the thread-local WorkflowCtx, and returns a panic-safe Drop guard that reaps the
    // previous scope. `set_workflow_scope` reads the SEMA_WORKFLOW_RUN_ID /
    // SEMA_WORKFLOW_FIXED_TS test seam internally.
    let guard = context::set_workflow_scope(&name, &doc, &meta, task_context)
        .map_err(|e| SemaError::eval(format!("workflow/run: {e}")))?;

    // run.started — emitted inside the live scope so seq starts at 0.
    {
        let ctx = context::current_for(task_context)
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

    // ── Implicit :mcp auth-resolution step, before the body thunk ─────────
    // A workflow with no :mcp meta key parses to an empty Vec here (O(1) on the absent
    // key), so every branch below is skipped and the body runs exactly as it did before
    // this feature — byte-identical for the no-:mcp case.
    let decls = match workflow_mcp::declared_mcp(&meta) {
        Ok(d) => d,
        Err(e) => {
            let ctx = context::current_for(task_context)
                .ok_or_else(|| SemaError::eval("workflow/run: scope not established"))?;
            let envelope = failed_envelope(&e.to_string());
            let (envelope, ack) = end_run_before_body(
                &ctx,
                guard,
                "failed",
                "mcp declaration invalid".to_string(),
                envelope,
            );
            return Ok(terminal_plan(task_context, envelope, ack));
        }
    };

    // A workflow with no `:mcp` runs the body straight away — byte-identical to the
    // pre-feature path.
    if decls.is_empty() {
        return Ok(ThunkPlan::Run {
            thunk,
            teardown: RunTeardown { guard, mcp: None },
        });
    }

    let run_id = {
        let ctx = context::current_for(task_context)
            .ok_or_else(|| SemaError::eval("workflow/run: scope not established"))?;
        ctx.set_mcp_declared(decls.iter().map(|d| d.alias.clone()).collect());
        ctx.run_id()
    };

    let Some(resolver) = workflow_mcp::workflow_mcp_resolver() else {
        let ctx = context::current_for(task_context)
            .ok_or_else(|| SemaError::eval("workflow/run: scope not established"))?;
        let envelope = failed_envelope(
            "workflow declares :mcp servers but this build has no MCP resolver registered",
        );
        let (envelope, ack) = end_run_before_body(
            &ctx,
            guard,
            "failed",
            "mcp resolution failed".to_string(),
            envelope,
        );
        return Ok(terminal_plan(task_context, envelope, ack));
    };

    if task_context.is_some() {
        // Runtime arm (inside a quantum): the real resolver's OAuth/connect I/O would hit
        // `io_block_on`'s active-quantum guard, so offload it structurally — the blocking
        // resolve runs on a plain worker; `ResolveContinuation` resumes here with the
        // encoded resolutions and runs the same gate/event logic as the host arm.
        let prepared = resolver.resolve_prepared(&decls, &name, &run_id);
        return Ok(ThunkPlan::Suspend(NativeSuspend {
            wait: WaitKind::External(prepared),
            continuation: Box::new(ResolveContinuation {
                guard,
                thunk,
                resolver,
            }),
        }));
    }

    // Host arm (outside a runtime quantum): `io_block_on` is legal — resolve inline.
    let resolutions = resolver.resolve(&decls, &name, &run_id);
    match apply_resolutions(task_context, guard, thunk, resolver, resolutions)? {
        ResolveGate::Exit { envelope, ack } => Ok(terminal_plan(task_context, envelope, ack)),
        ResolveGate::Proceed { thunk, teardown } => Ok(ThunkPlan::Run { thunk, teardown }),
    }
}

/// The outcome of the shared `:mcp` gate/event logic ([`apply_resolutions`]): either the
/// run ended before the body (an envelope to return) or every declared server resolved
/// and the body thunk should run with its teardown state.
enum ResolveGate {
    Exit { envelope: Value, ack: Receiver<()> },
    Proceed { thunk: Value, teardown: RunTeardown },
}

/// Emit the per-server auth events, apply the Failed-over-NeedsAuth gate precedence, and
/// either end the run before the body (Failed/NeedsAuth) or publish the connected handles
/// and hand back the body thunk + teardown. Shared verbatim by the host arm (synchronous
/// resolve) and the runtime arm's [`ResolveContinuation`] (offloaded resolve), so the two
/// never diverge. Consumes `guard`: an `Exit` drops it via `end_run_before_body`; a
/// `Proceed` moves it into the `RunTeardown`.
fn apply_resolutions(
    task_context: Option<&TaskContextHandle>,
    guard: context::WorkflowGuard,
    thunk: Value,
    resolver: Rc<dyn WorkflowMcpResolver>,
    resolutions: Vec<ServerResolution>,
) -> Result<ResolveGate, SemaError> {
    let ctx = context::current_for(task_context)
        .ok_or_else(|| SemaError::eval("workflow/run: scope not established"))?;

    let mut connected: BTreeMap<String, Value> = BTreeMap::new();
    let mut connected_handles: Vec<Value> = Vec::new();
    let mut needs_auth: Vec<(String, String, String)> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();

    // Emit events per resolution IN THE GIVEN (alias-sorted) order — the resolver returns
    // them in the same order as `decls`.
    for resolution in &resolutions {
        match resolution {
            ServerResolution::Connected {
                alias,
                handle,
                auth,
            } => {
                connected.insert(alias.clone(), handle.clone());
                connected_handles.push(handle.clone());
                if let Some(grant) = auth {
                    ctx.emit(WorkflowEvent::AuthGranted {
                        seq: ctx.next_seq(),
                        ts: ctx.ts(),
                        server: alias.clone(),
                        scopes: grant.scopes.clone(),
                        expires_at: grant.expires_at,
                        source: grant.source.clone(),
                    });
                }
            }
            ServerResolution::NeedsAuth {
                alias,
                url,
                scopes,
                tools,
                persist,
            } => {
                ctx.emit(WorkflowEvent::AuthRequired {
                    seq: ctx.next_seq(),
                    ts: ctx.ts(),
                    server: alias.clone(),
                    scopes: scopes.clone(),
                    tools: tools.clone(),
                    persist: persist.clone(),
                });
                needs_auth.push((alias.clone(), url.clone(), persist.clone()));
            }
            ServerResolution::Failed { alias, reason } => {
                ctx.emit(WorkflowEvent::AuthFailed {
                    seq: ctx.next_seq(),
                    ts: ctx.ts(),
                    server: alias.clone(),
                    reason: reason.clone(),
                });
                failures.push((alias.clone(), reason.clone()));
            }
        }
    }

    // Outcome precedence: any Failed wins over any NeedsAuth. Both close whatever DID
    // connect before ending the run — the body NEVER runs.
    if !failures.is_empty() {
        resolver.close(&connected_handles);
        let msg = failures
            .iter()
            .map(|(alias, reason)| format!("{alias}: {reason}"))
            .collect::<Vec<_>>()
            .join("; ");
        let envelope = failed_envelope(&msg);
        let (envelope, ack) = end_run_before_body(
            &ctx,
            guard,
            "failed",
            "mcp resolution failed".to_string(),
            envelope,
        );
        return Ok(ResolveGate::Exit { envelope, ack });
    }
    if !needs_auth.is_empty() {
        resolver.close(&connected_handles);
        let envelope = needs_auth_envelope(&needs_auth);
        let (envelope, ack) = end_run_before_body(
            &ctx,
            guard,
            "needs-auth",
            "authentication required".to_string(),
            envelope,
        );
        return Ok(ResolveGate::Exit { envelope, ack });
    }

    // Every declared server connected: publish handles for workflow/mcp-handle, and
    // remember (resolver, handles) so `finish_run` closes them EXACTLY once.
    ctx.set_mcp_handles(connected);
    Ok(ResolveGate::Proceed {
        thunk,
        teardown: RunTeardown {
            guard,
            mcp: Some(McpClose {
                resolver,
                handles: connected_handles,
            }),
        },
    })
}

/// Trace the `Value` edges a pending `workflow/run` teardown carries — its open MCP
/// handles. Shared by `register`'s trace hook and [`ResolveContinuation`]'s downstream
/// `ThunkContinuation`.
fn trace_run_teardown(teardown: &RunTeardown, sink: &mut dyn FnMut(GcEdge<'_>)) {
    if let Some(mcp) = &teardown.mcp {
        for handle in &mcp.handles {
            sink(GcEdge::Value(handle));
        }
    }
}

/// The runtime arm's resume point for `workflow/run`'s offloaded `:mcp` resolve. Parked
/// on the `resolve_prepared` External wait, it holds the run's scope `guard`, body
/// `thunk`, and `resolver` across the suspension; on completion it decodes the resolutions
/// `Value` and runs the shared gate/event logic, then either returns the gate envelope or
/// drives the body thunk (`NativeOutcome::Call`) with the run teardown.
///
/// CORE-2 I2: the only `Value` it retains is `thunk`, which its `trace` emits. Cancellation
/// or a worker failure tears the run down (drops `guard`, so the scope token is removed;
/// any half-open connection was dropped inside the worker's drop-on-cancel select).
struct ResolveContinuation {
    guard: context::WorkflowGuard,
    thunk: Value,
    resolver: Rc<dyn WorkflowMcpResolver>,
}

impl Trace for ResolveContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.thunk));
        true
    }
}

impl NativeContinuation for ResolveContinuation {
    fn resume(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let task_context = context.task_context.clone();
        let ResolveContinuation {
            guard,
            thunk,
            resolver,
        } = *self;
        match input {
            ResumeInput::Returned(value) => {
                let resolutions = workflow_mcp::decode_resolutions(&value);
                match apply_resolutions(Some(&task_context), guard, thunk, resolver, resolutions)? {
                    ResolveGate::Exit { envelope, ack } => Ok(NativeOutcome::Suspend(
                        build_flush_ack_suspend(envelope, ack),
                    )),
                    ResolveGate::Proceed { thunk, teardown } => {
                        Ok(NativeOutcome::Call(NativeCall {
                            callable: thunk,
                            args: Vec::new(),
                            continuation: Box::new(ThunkContinuation {
                                teardown: Some(teardown),
                                finish: finish_run,
                                trace_teardown: trace_run_teardown,
                                name: "workflow/run",
                            }),
                        }))
                    }
                }
            }
            // A worker-level failure (panic / bound): end the run as failed so it is
            // journaled, mirroring a `ServerResolution::Failed` gate rather than leaving
            // the scope dangling.
            ResumeInput::Failed(error) => {
                let ctx = context::current_for(Some(&task_context))
                    .ok_or_else(|| SemaError::eval("workflow/run: scope not established"))?;
                let envelope = failed_envelope(&error.to_string());
                let (envelope, ack) = end_run_before_body(
                    &ctx,
                    guard,
                    "failed",
                    "mcp resolution failed".to_string(),
                    envelope,
                );
                Ok(NativeOutcome::Suspend(build_flush_ack_suspend(
                    envelope, ack,
                )))
            }
            // Cancellation reaps the run: dropping `guard` removes the scope token; the
            // worker already dropped any half-open connection. Propagate the cancellation.
            ResumeInput::Cancelled(reason) => {
                drop(guard);
                Err(SemaError::eval(format!(
                    "workflow/run MCP resolution was cancelled ({reason:?})"
                )))
            }
            ResumeInput::Runtime(_) => {
                drop(guard);
                Err(SemaError::eval(
                    "workflow/run: unexpected runtime response after MCP resolution",
                ))
            }
        }
    }
}

pub fn register(env: &sema_core::Env) {
    // (workflow/run name doc meta thunk) — open a run scope, journal start/end, return
    // the {:status ...} envelope. `name`/`doc` are strings; `meta` is the workflow's
    // metadata map ({:args ... :budget ... :permissions ...}); `thunk` is the (lambda () ...)
    // wrapping the workflow body.
    register_thunk_fn(
        env,
        "workflow/run",
        run_plan,
        finish_run,
        trace_run_teardown,
    );

    // (workflow/phase label) — a MARKER (workflow.js semantics), not a wrapper. Closes
    // the previously-open phase (emitting its phase.ended) then opens `label`. The
    // checkpoints/agents that follow attribute to this phase until the next marker or
    // the run end (`workflow/run` closes the last open phase). Returns nil.
    register_scoped_fn(env, "workflow/phase", |task_context, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("workflow/phase", "1", args.len()));
        }
        let label = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let ctx = context::current_for(task_context)
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

    // (workflow/step label thunk) — run a leaf (typically an LLM/tool call) as a
    // journaled "step": emits agent.started before and agent.result after (the
    // event vocabulary is the frozen internal contract — `agent.*` names predate
    // the step rename and stay), so the dashboard renders it as a row under the
    // current phase. Returns the thunk's value (or propagates its error, after
    // journaling a result). A no-op wrapper (just runs the thunk) when called
    // outside a workflow run. See `step_plan` / `finish_step`.
    register_thunk_fn(
        env,
        "workflow/step",
        step_plan,
        finish_step,
        |_teardown, _sink| {
            // `StepTeardown` carries only scalar/`Rc<RefCell<LeafUsage>>` state — no `Value`.
        },
    );

    // (workflow/tool-call tool-name [args]) — journal a tool call by the current
    // agent (the dashboard renders these as tool twigs in the agent's drill-in).
    // No-op (returns nil) outside a workflow/step. `args` is an opaque/gated
    // descriptor; pass a string or omit for the "gated" sentinel.
    register_scoped_fn(env, "workflow/tool-call", |task_context, args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("workflow/tool-call", "1-2", args.len()));
        }
        let tool_name = as_name(&args[0])
            .ok_or_else(|| SemaError::type_error("keyword or string", args[0].type_name()))?;
        if let Some(ctx) = context::current_for(task_context) {
            if let Some(agent_id) = context::cur_agent_for(task_context) {
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

    // (workflow/checkpoint :k thunk) records+returns (thunk) and emits a checkpoint
    // event; (workflow/checkpoint :k) reads the stored value (nil if unset). The
    // public (checkpoint :k v) macro delays v into the thunk, so a resume memo hit can
    // skip evaluating the write expression entirely.
    register_thunk_fn(
        env,
        "workflow/checkpoint",
        checkpoint_plan,
        finish_checkpoint,
        |_teardown, _sink| {
            // Checkpoint teardown retains only the key and content-key strings.
        },
    );

    // (workflow/mcp-handle alias) — the resolved MCP connection handle for a
    // declared `:mcp` alias (a symbol or keyword). Only meaningful once
    // workflow/run's implicit auth-resolution step has completed: the
    // `defworkflow` macro's generated `(let ((asana (workflow/mcp-handle
    // (quote asana))) …) ,@body)` bindings live INSIDE the body thunk, which
    // workflow/run only invokes after every declared server resolves to
    // Connected — so that call site is always safe. A direct/manual call
    // site (not the macro-generated one) is still handled defensively below.
    register_scoped_fn(env, "workflow/mcp-handle", |task_context, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("workflow/mcp-handle", "1", args.len()));
        }
        let alias = args[0]
            .as_symbol()
            .or_else(|| args[0].as_keyword())
            .ok_or_else(|| {
                SemaError::type_error("symbol or keyword", args[0].type_name())
                    .with_hint("e.g. (workflow/mcp-handle 'asana) or (workflow/mcp-handle :asana)")
            })?;
        let ctx = context::current_for(task_context).ok_or_else(|| {
            SemaError::eval("workflow/mcp-handle outside a workflow/run")
                .with_hint("call workflow/mcp-handle from inside a running workflow's body")
        })?;
        if let Some(handle) = ctx.mcp_handle(&alias) {
            return Ok(handle);
        }
        if ctx.is_mcp_declared(&alias) {
            return Err(SemaError::eval(format!(
                "workflow/mcp-handle: `{alias}` is declared but this run has not resolved its \
                 :mcp servers yet"
            ))
            .with_hint(
                "workflow/mcp-handle is only valid inside the workflow body, after the \
                 implicit auth-resolution step",
            ));
        }
        Err(SemaError::eval(format!(
            "workflow/mcp-handle: `{alias}` is not declared in this workflow's :mcp meta"
        ))
        .with_hint("declare it in :mcp {alias {...}} in the defworkflow meta map"))
    });

    // (workflow/check src) — static-check a workflow source string (or any value,
    // which is pretty-printed to source) and return diagnostics as a Sema list of
    // maps. An empty list means the source is clean. Each map has:
    //   {:severity <:error|:warning> :code "E-PHASE-ARITY" :message "..." :line N :col N :hint "..."}
    // :line and :col are nil when no span is available. Calls the pure checker in
    // workflow_check — no eval, no LLM, no I/O.
    crate::register_fn(env, "workflow/check", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("workflow/check", "1", args.len()));
        }
        let src: String = if let Some(s) = args[0].as_str() {
            s.to_string()
        } else {
            sema_core::pretty_print(&args[0], 100)
        };
        let diags = crate::workflow_check::check_source(&src);
        let items: Vec<Value> = diags
            .iter()
            .map(|d| {
                let mut m = BTreeMap::new();
                m.insert(
                    Value::keyword("severity"),
                    match d.severity {
                        crate::workflow_check::Severity::Error => Value::keyword("error"),
                        crate::workflow_check::Severity::Warning => Value::keyword("warning"),
                    },
                );
                m.insert(Value::keyword("code"), Value::string(d.code));
                m.insert(Value::keyword("message"), Value::string(&d.message));
                m.insert(
                    Value::keyword("line"),
                    d.span
                        .map(|s| Value::int(s.line as i64))
                        .unwrap_or_else(Value::nil),
                );
                m.insert(
                    Value::keyword("col"),
                    d.span
                        .map(|s| Value::int(s.col as i64))
                        .unwrap_or_else(Value::nil),
                );
                m.insert(
                    Value::keyword("hint"),
                    d.hint
                        .as_deref()
                        .map(Value::string)
                        .unwrap_or_else(Value::nil),
                );
                Value::map(m)
            })
            .collect();
        Ok(Value::list(items))
    });
}

#[cfg(test)]
mod continuation_tests {
    use super::*;

    /// A `ThunkContinuation` must expose exactly the GC edges its teardown state carries
    /// (a run's open MCP handles are the only `Value`s any workflow teardown holds; a
    /// step's teardown holds none), and none once the teardown has been taken.
    #[test]
    fn thunk_continuation_traces_teardown_value_edges() {
        use sema_core::runtime::Trace;
        // Stand-in teardown that carries two `Value` handles, traced like `RunTeardown`.
        let cont = ThunkContinuation::<Vec<Value>> {
            teardown: Some(vec![Value::int(1), Value::int(2)]),
            finish: |_tc, _t, r, _d| r.map(NativeOutcome::Return),
            trace_teardown: |t: &Vec<Value>, sink: &mut dyn FnMut(GcEdge<'_>)| {
                for v in t {
                    sink(GcEdge::Value(v));
                }
            },
            name: "workflow/test",
        };
        let mut edges = 0usize;
        assert!(cont.trace(&mut |_| edges += 1));
        assert_eq!(edges, 2, "must expose one edge per teardown-held handle");

        // A step-shaped teardown (no `Value`) exposes zero edges.
        let empty = ThunkContinuation::<()> {
            teardown: Some(()),
            finish: |_tc, _t, r, _d| r.map(NativeOutcome::Return),
            trace_teardown: |_t, _sink| {},
            name: "workflow/test",
        };
        let mut edges = 0usize;
        assert!(empty.trace(&mut |_| edges += 1));
        assert_eq!(edges, 0, "a teardown with no Value must expose no edges");

        let checkpoint = ThunkContinuation::<CheckpointTeardown> {
            teardown: Some(CheckpointTeardown {
                key: "key".into(),
                resume_key: "content-key".into(),
            }),
            finish: finish_checkpoint,
            trace_teardown: |_teardown, _sink| {},
            name: "workflow/checkpoint",
        };
        let mut edges = 0usize;
        assert!(checkpoint.trace(&mut |_| edges += 1));
        assert_eq!(edges, 0, "checkpoint teardown must expose no Value edges");
    }
}
