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
    NativeCall, NativeCallContext, NativeContinuation, NativeOutcome, NativeResult, ResumeInput,
    Trace,
};
use sema_core::{SemaError, Value};
use sema_workflow::context;
use sema_workflow::event::WorkflowEvent;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Instant;

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

/// Render a value for the journal so the dashboard can show the real data, capped via
/// [`cap_text`].
fn capped_render(v: &Value) -> String {
    cap_text(&sema_core::pretty_print(v, 100))
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
) -> Value {
    close_open_phase(ctx, status);
    ctx.emit(WorkflowEvent::RunEnded {
        seq: ctx.next_seq(),
        ts: ctx.ts(),
        status: status.into(),
        reason: Some(reason),
        dur_ms: ctx.dur_ms(),
    });
    ctx.write_result(&envelope);
    drop(guard);
    envelope
}

/// Post-thunk teardown for a `register_thunk_fn` native: given the teardown state and
/// the thunk's result, journal/close and return the builtin's value.
type FinishFn<T> = fn(T, Result<Value, SemaError>) -> Result<Value, SemaError>;
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
    plan: impl Fn(&[Value]) -> Result<ThunkPlan<T>, SemaError> + 'static,
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
            move |args| match for_legacy(args)? {
                ThunkPlan::Immediate(value) => Ok(value),
                ThunkPlan::Run { thunk, teardown } => {
                    let result = crate::list::call_function(&thunk, &[]);
                    finish(teardown, result)
                }
            },
            move |_ctx, args| match for_runtime(args)? {
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
            },
        )),
    );
}

/// A workflow builtin's pre-thunk decision: either an immediate value (nothing to run —
/// a resume replay, a budget-tripped skip, or a pre-body `:mcp` gate exit) or a thunk to
/// run with the `teardown` state its `finish` needs afterward.
enum ThunkPlan<T> {
    Immediate(Value),
    Run { thunk: Value, teardown: T },
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
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let result = match input {
            ResumeInput::Returned(value) => Ok(value),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "{} thunk was cancelled ({reason:?})",
                self.name
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
                "{} teardown received an unexpected runtime response",
                self.name
            ))),
        };
        let teardown = self
            .teardown
            .take()
            .expect("thunk continuation resumed once");
        (self.finish)(teardown, result).map(NativeOutcome::Return)
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
    teardown: Option<StepTeardown>,
    result: Result<Value, SemaError>,
) -> Result<Value, SemaError> {
    let Some(td) = teardown else {
        // Transparent (outside a run): nothing to journal.
        return result;
    };
    let Some(ctx) = context::current() else {
        return result;
    };
    ctx.set_cur_agent(None);
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
    result
}

/// Pre-thunk work for `workflow/step` — see the original inline documentation preserved
/// in `finish_step` and the event emissions below.
fn step_plan(args: &[Value]) -> Result<ThunkPlan<Option<StepTeardown>>, SemaError> {
    if args.len() != 2 {
        return Err(SemaError::arity("workflow/step", "2", args.len()));
    }
    // First arg is the agent role: an opts map `{:name "scout" …}` (the `agent` macro
    // form), or a bare keyword/string label. Default role is "agent".
    let label = agent_role(&args[0]);
    let thunk = args[1].clone();
    let Some(ctx) = context::current() else {
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
    // to it; cleared in `finish_step`. `cur_agent` is a single shared slot — under a
    // CONCURRENT `:tools` fan-out attribution is best-effort (same single-slot caveat as
    // the per-agent budget).
    ctx.set_cur_agent(Some(agent_id.clone()));
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

/// Post-thunk teardown state for `workflow/run`. Holds the scope guard (dropped last,
/// after `run.ended` + `result.json`) and any open MCP handles to close (the handles are
/// `Value`s — traced).
struct RunTeardown {
    guard: context::WorkflowGuard,
    mcp_resolver: Option<Rc<dyn WorkflowMcpResolver>>,
    mcp_handles: Vec<Value>,
}

/// Derive the run envelope from the body's result, journal `run.ended`, write
/// `result.json`, close any MCP handles, then drop the scope guard. Shared by the legacy
/// and cooperative paths; always returns an envelope (a body error becomes a failed one).
fn finish_run(teardown: RunTeardown, result: Result<Value, SemaError>) -> Result<Value, SemaError> {
    let RunTeardown {
        guard,
        mcp_resolver,
        mcp_handles,
    } = teardown;
    let (mut status, mut envelope, mut reason) = match &result {
        Ok(v) => ("success", success_envelope(v.clone()), None),
        Err(e) => (
            "failed",
            failed_envelope(&e.to_string()),
            Some("workflow body returned an error".to_string()),
        ),
    };
    // Close any resolved MCP handles exactly once, regardless of how the body exited.
    if let Some(resolver) = mcp_resolver {
        resolver.close(&mcp_handles);
    }
    if let Some(ctx) = context::current() {
        // A tripped budget cap fails the run regardless of the body's own outcome.
        if ctx.over_budget() {
            status = "failed";
            envelope = budget_failed_envelope();
            reason = Some("budget exceeded".to_string());
        }
        close_open_phase(&ctx, status);
        ctx.emit(WorkflowEvent::RunEnded {
            seq: ctx.next_seq(),
            ts: ctx.ts(),
            status: status.into(),
            reason,
            dur_ms: ctx.dur_ms(),
        });
        ctx.write_result(&envelope);
    }
    drop(guard);
    Ok(envelope)
}

/// Pre-thunk work for `workflow/run`: open the run scope, journal `run.started`, resolve
/// any declared `:mcp` servers (a pre-body gate that can end the run before the body ever
/// runs), and hand back the body thunk plus the teardown state. Mirrors the original
/// inline builtin; the post-body work moved to `finish_run`.
fn run_plan(args: &[Value]) -> Result<ThunkPlan<RunTeardown>, SemaError> {
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

    // ── Implicit :mcp auth-resolution step, before the body thunk ─────────
    // A workflow with no :mcp meta key parses to an empty Vec here (O(1) on the absent
    // key), so every branch below is skipped and the body runs exactly as it did before
    // this feature — byte-identical for the no-:mcp case.
    let decls = match workflow_mcp::declared_mcp(&meta) {
        Ok(d) => d,
        Err(e) => {
            let ctx = context::current()
                .ok_or_else(|| SemaError::eval("workflow/run: scope not established"))?;
            let envelope = failed_envelope(&e.to_string());
            return Ok(ThunkPlan::Immediate(end_run_before_body(
                &ctx,
                guard,
                "failed",
                "mcp declaration invalid".to_string(),
                envelope,
            )));
        }
    };

    // Handles resolved below (if any), closed exactly once after the body exits.
    let mut mcp_resolver: Option<Rc<dyn WorkflowMcpResolver>> = None;
    let mut mcp_handles: Vec<Value> = Vec::new();

    if !decls.is_empty() {
        let ctx = context::current()
            .ok_or_else(|| SemaError::eval("workflow/run: scope not established"))?;
        ctx.set_mcp_declared(decls.iter().map(|d| d.alias.clone()).collect());

        let Some(resolver) = workflow_mcp::workflow_mcp_resolver() else {
            let envelope = failed_envelope(
                "workflow declares :mcp servers but this build has no MCP resolver \
                 registered",
            );
            return Ok(ThunkPlan::Immediate(end_run_before_body(
                &ctx,
                guard,
                "failed",
                "mcp resolution failed".to_string(),
                envelope,
            )));
        };

        let resolutions = resolver.resolve(&decls, &name, &ctx.run_id());

        let mut connected: BTreeMap<String, Value> = BTreeMap::new();
        let mut connected_handles: Vec<Value> = Vec::new();
        let mut needs_auth: Vec<(String, String, String)> = Vec::new();
        let mut failures: Vec<(String, String)> = Vec::new();

        // Emit events per resolution IN THE GIVEN (alias-sorted) order — the resolver
        // returns them in the same order as `decls`.
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
            return Ok(ThunkPlan::Immediate(end_run_before_body(
                &ctx,
                guard,
                "failed",
                "mcp resolution failed".to_string(),
                envelope,
            )));
        }
        if !needs_auth.is_empty() {
            resolver.close(&connected_handles);
            let envelope = needs_auth_envelope(&needs_auth);
            return Ok(ThunkPlan::Immediate(end_run_before_body(
                &ctx,
                guard,
                "needs-auth",
                "authentication required".to_string(),
                envelope,
            )));
        }

        // Every declared server connected: publish handles for workflow/mcp-handle, and
        // remember (resolver, handles) so `finish_run` closes them EXACTLY once.
        ctx.set_mcp_handles(connected);
        mcp_resolver = Some(resolver);
        mcp_handles = connected_handles;
    }

    Ok(ThunkPlan::Run {
        thunk,
        teardown: RunTeardown {
            guard,
            mcp_resolver,
            mcp_handles,
        },
    })
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
        |teardown: &RunTeardown, sink: &mut dyn FnMut(GcEdge<'_>)| {
            // The only GC-visible state a pending run holds is its open MCP handles.
            for handle in &teardown.mcp_handles {
                sink(GcEdge::Value(handle));
            }
        },
    );

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

    // (workflow/checkpoint :k thunk) records+returns (thunk) and emits a checkpoint
    // event; (workflow/checkpoint :k) reads the stored value (nil if unset). The
    // public (checkpoint :k v) macro delays v into the thunk, so a resume memo hit can
    // skip evaluating the write expression entirely.
    crate::register_fn(env, "workflow/checkpoint", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("workflow/checkpoint", "1-2", args.len()));
        }
        let key = as_name(&args[0])
            .ok_or_else(|| SemaError::type_error("keyword or string", args[0].type_name()))?;
        let ctx = context::current()
            .ok_or_else(|| SemaError::eval("checkpoint outside a workflow/run"))?;

        if args.len() == 2 {
            // Resume short-circuit: a memoized checkpoint returns the recorded value,
            // seeds the state bag (so later `(checkpoint :k)` reads see it), and skips
            // evaluating the write thunk / re-emitting the checkpoint event. The key
            // advances its occurrence ordinal on every run (computed before the resume
            // check) to stay in body order.
            let resume_key = ctx.checkpoint_content_key(&key, &ctx.cur_phase_label());
            if ctx.resuming() {
                if let Some(v) = ctx.memo_lookup(&resume_key) {
                    ctx.store_checkpoint(&key, v.clone());
                    return Ok(v);
                }
            }
            // Write miss: evaluate, store, journal, return the value (so it threads
            // through `let`).
            let value = crate::list::call_function(&args[1], &[])?;
            ctx.store_checkpoint(&key, value.clone());
            let digest = ctx.value_digest(&value);
            ctx.emit(WorkflowEvent::Checkpoint {
                seq: ctx.next_seq(),
                ts: ctx.ts(),
                phase_seq: ctx.phase_seq(),
                key: key.clone(),
                content_key: resume_key.clone(),
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

    // (workflow/mcp-handle alias) — the resolved MCP connection handle for a
    // declared `:mcp` alias (a symbol or keyword). Only meaningful once
    // workflow/run's implicit auth-resolution step has completed: the
    // `defworkflow` macro's generated `(let ((asana (workflow/mcp-handle
    // (quote asana))) …) ,@body)` bindings live INSIDE the body thunk, which
    // workflow/run only invokes after every declared server resolves to
    // Connected — so that call site is always safe. A direct/manual call
    // site (not the macro-generated one) is still handled defensively below.
    crate::register_fn(env, "workflow/mcp-handle", |args| {
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
        let ctx = context::current().ok_or_else(|| {
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
            finish: |_t, r| r,
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
            finish: |_t, r| r,
            trace_teardown: |_t, _sink| {},
            name: "workflow/test",
        };
        let mut edges = 0usize;
        assert!(empty.trace(&mut |_| edges += 1));
        assert_eq!(edges, 0, "a teardown with no Value must expose no edges");
    }
}
