use super::*;

// Build the `:usage` map for `agent/run`'s result (both the blocking and non-blocking
// paths), from a `LeafUsage` tally accumulated over a `UsageScope`. Same key names as
// `llm/last-usage`/`llm/session-usage` for consistency, plus `:calls` — the number of
// billed provider round trips this turn, which neither of those two (single-call /
// process-global) accessors can report.
pub(super) fn agent_usage_value(u: &LeafUsage) -> Value {
    let mut map = BTreeMap::new();
    map.insert(
        Value::keyword("prompt-tokens"),
        Value::int(u.input_tokens as i64),
    );
    map.insert(
        Value::keyword("completion-tokens"),
        Value::int(u.output_tokens as i64),
    );
    map.insert(
        Value::keyword("total-tokens"),
        Value::int((u.input_tokens + u.output_tokens) as i64),
    );
    map.insert(
        Value::keyword("cache-read-tokens"),
        Value::int(u.cache_read_input_tokens as i64),
    );
    map.insert(
        Value::keyword("cache-creation-tokens"),
        Value::int(u.cache_creation_input_tokens as i64),
    );
    map.insert(Value::keyword("model"), Value::string(&u.model));
    if let Some(cost) = u.cost_usd {
        map.insert(Value::keyword("cost-usd"), Value::float(cost));
    }
    map.insert(Value::keyword("calls"), Value::int(u.calls as i64));
    Value::map(map)
}

/// Build ToolSchema list from Sema ToolDef values.
pub(super) fn build_tool_schemas(tools: &[Value]) -> Result<Vec<ToolSchema>, SemaError> {
    let mut schemas = Vec::new();
    for tool in tools {
        let td = tool
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", tool.type_name()))?;
        let params_json = sema_core::value_to_json_schema(&td.parameters);
        schemas.push(ToolSchema {
            name: td.name.clone(),
            description: td.description.clone(),
            parameters: params_json,
        });
    }
    Ok(schemas)
}

pub(super) fn tool_policy_subject_to_value(subject: &sema_core::ToolPolicySubject) -> Value {
    let mut map = BTreeMap::new();
    match subject {
        sema_core::ToolPolicySubject::File { access, path_arg } => {
            let kind = match access {
                sema_core::FileAccess::Read => "file-read",
                sema_core::FileAccess::Write => "file-write",
                sema_core::FileAccess::Delete => "file-delete",
            };
            map.insert(Value::keyword("kind"), Value::keyword(kind));
            map.insert(Value::keyword("path-arg"), Value::keyword(path_arg));
        }
        sema_core::ToolPolicySubject::NetworkRequest { method, url_arg } => {
            map.insert(Value::keyword("kind"), Value::keyword("network-request"));
            map.insert(Value::keyword("url-arg"), Value::keyword(url_arg));
            if let Some(method) = method {
                map.insert(Value::keyword("method"), Value::string(method));
            }
        }
        sema_core::ToolPolicySubject::Command { command_arg } => {
            map.insert(Value::keyword("kind"), Value::keyword("command"));
            map.insert(Value::keyword("command-arg"), Value::keyword(command_arg));
        }
        sema_core::ToolPolicySubject::ExternalAction { action, target_arg } => {
            map.insert(Value::keyword("kind"), Value::keyword("external-action"));
            map.insert(Value::keyword("action"), Value::keyword(action));
            if let Some(target_arg) = target_arg {
                map.insert(Value::keyword("target-arg"), Value::keyword(target_arg));
            }
        }
    }
    Value::map(map)
}

/// Bound runaway error loops across the agent conversation (mirrors `run_tool_loop`).
pub(super) const MAX_CONSECUTIVE_TOOL_ERRORS: usize = 5;

/// Per-run state for the non-blocking runtime agent loop. Lives in the thread-local
/// `AGENT_RUNS` slab keyed by an integer token handed to Sema, so it survives every
/// inter-round and inter-tool suspension (the slab is on the VM thread; nothing
/// here is `Send` and nothing crosses threads). No `__agent-*`
/// native holds a `RefCell` borrow of the slab across a callback / tool execution /
/// completion suspension — each short-borrows to copy inputs out, drops, does the work,
/// then short-borrows again to write back.
pub(super) struct AgentLoopState {
    messages: Vec<ChatMessage>,
    tools: Vec<Value>,
    tool_schemas: Vec<ToolSchema>,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    system: Option<String>,
    reasoning_effort: Option<String>,
    on_tool_call: Option<Value>,
    on_text: Option<Value>,
    round: usize,
    max_rounds: usize,
    consecutive_errors: usize,
    pending_tool_calls: Vec<ToolCall>,
    last_content: String,
    first_input: String,
    /// Set once the loop should stop (no tool calls, round cap, or consec-error abort).
    done: bool,
    /// Non-empty error message when the run aborted (consecutive tool errors); raised
    /// by `__agent-finish` so the abort surfaces to the caller like the blocking path.
    abort_error: Option<String>,
    /// Whether a final plain-assistant message has been appended to `messages`.
    final_pushed: bool,
    output_conv_id: String,
    has_opts: bool,
    memory_handle: Option<Value>,
    pre_user_count: usize,
    agent_model: String,
    /// The attached agent OTel span (pushed on the thread-local stack in `__agent-begin`,
    /// popped+ended when this state is removed from the slab). `Option` so the custom
    /// `Drop` can forget it when the otel thread-locals are already gone (see below).
    agent_span: Option<sema_otel::AgentSpan>,
    conv_guard: Option<sema_otel::ConversationGuard>,
    /// This turn's usage tally (issue #86), read by `__agent-finish` for the `:usage`
    /// result key. `Some` for `agent/run` (opened in `agent_begin`), `None` for
    /// `llm/chat` (`chat_begin` always sets `has_opts: false`, so no map — and no
    /// `:usage` — is ever built from this state). `Option` for the same reason as
    /// `agent_span`/`conv_guard`: `Drop` must be able to `.take()` it to choose
    /// drop-normally vs. forget-at-thread-teardown.
    usage_scope: Option<UsageScope>,
    /// The scheduler task this run's driver loop executes on (captured in
    /// `__agent-begin`); `None` for a top-level (non-task) run. When that task is
    /// CANCELLED its bytecode never resumes, so `__agent-finish` never fires —
    /// the task-reaped sweep (`reap_cancelled_agent_runs`) matches on this id to
    /// reclaim the entry (and end its span) instead of leaking it until
    /// `reset_runtime_state`.
    owning_task_id: Option<RuntimeTaskId>,
}

impl Drop for AgentLoopState {
    fn drop(&mut self) {
        // Normal path (`__agent-finish`, or `reset_runtime_state` during eval): the otel
        // thread-locals are live, so let the span guard pop+end and the scope guard
        // restore — dropping the span BEFORE the scope (reverse of begin's install order).
        if sema_otel::tls_alive() {
            drop(self.agent_span.take());
            drop(self.conv_guard.take());
        } else {
            // Thread teardown of a leaked (cancelled) run: the otel thread-locals are
            // already destroyed. Forget the guards rather than let their `Drop` touch
            // dead TLS and abort the process. The span never flushes, which is
            // acceptable for a cancelled run at process exit.
            std::mem::forget(self.agent_span.take());
            std::mem::forget(self.conv_guard.take());
        }
        // `usage_scope` touches a different (this crate's own) thread-local than the
        // otel guards above, which can be destroyed in a different order at thread
        // teardown — probe it independently rather than reusing `sema_otel::tls_alive()`.
        if usage_tls_alive() {
            drop(self.usage_scope.take());
        } else {
            std::mem::forget(self.usage_scope.take());
        }
    }
}

/// Whether `ACTIVE_LEAF_SCOPE` is still accessible on this thread. Mirrors
/// `sema_otel::tls_alive()`'s purpose for the otel thread-locals: lets a held
/// `UsageScope`'s `Drop` avoid touching an already-destroyed thread-local during
/// thread teardown (a leaked/cancelled `AgentLoopState` dropped at process exit).
pub(super) fn usage_tls_alive() -> bool {
    ACTIVE_LEAF_SCOPE.try_with(|_| ()).is_ok()
}

thread_local! {
    /// Live non-blocking agent runs, keyed by the integer token handed to Sema.
    pub(super) static AGENT_RUNS: RefCell<std::collections::HashMap<u64, AgentLoopState>> =
        RefCell::new(std::collections::HashMap::new());
    pub(super) static AGENT_RUN_NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

/// Clear any live agent-loop state (called from `reset_runtime_state`). Dropping the
/// entries ends any still-open agent spans; benign when otel is disabled.
pub(super) fn clear_agent_runs() {
    AGENT_RUNS.with(|r| r.borrow_mut().clear());
    AGENT_RUN_NEXT_ID.with(|c| c.set(1));
}

/// Test instrumentation: the number of live entries in the non-blocking agent-run
/// slab. A settled scheduler must leave this at 0 — normal exit and Sema errors go
/// through `__agent-finish`, and a cancelled task's entries are reclaimed by
/// [`reap_cancelled_agent_runs`].
pub fn agent_runs_len() -> usize {
    AGENT_RUNS.with(|r| r.borrow().len())
}

/// Test instrumentation: number of live stream-run slab entries.
pub fn stream_runs_len() -> usize {
    STREAM_RUNS.with(|r| r.borrow().len())
}

/// Task-reaped sweep (registered via `sema_core::set_task_reaped_callback`): when
/// the scheduler cancels a task it will never resume, remove every slab entry that
/// task owns. `__agent-finish` cannot run for a cancelled task (its bytecode is
/// gone), so this is the entry's only reclamation point before
/// `reset_runtime_state`. Runs on the VM thread with OTel TLS alive, but with the
/// CANCELLER's otel context installed — not the dead task's — so the span/scope
/// guards must not touch the installed stack/ids:
/// - the agent span ends via `end_unstacked` (its pushed context lives on the dead
///   task's saved span stack; a popping end would mis-pop the canceller's stack);
/// - the conversation guard is `defuse`d (restoring its saved prev ids would
///   clobber the canceller's).
///
/// Idempotent by absence in both directions: after `__agent-finish` removed the
/// entry this sweep finds nothing, and a late finish after this sweep is the
/// existing idempotent no-op. Entries with `owning_task_id: None` are untouched.
pub(super) fn reap_cancelled_agent_runs(task_id: RuntimeTaskId) {
    let reaped: Vec<AgentLoopState> = AGENT_RUNS.with(|r| {
        let mut slab = r.borrow_mut();
        let tokens: Vec<u64> = slab
            .iter()
            .filter(|(_, st)| st.owning_task_id == Some(task_id))
            .map(|(k, _)| *k)
            .collect();
        tokens.into_iter().filter_map(|t| slab.remove(&t)).collect()
    });
    for mut st in reaped {
        if let Some(span) = st.agent_span.take() {
            span.record_error("cancelled", "agent run cancelled");
            span.end_unstacked();
        }
        if let Some(guard) = st.conv_guard.take() {
            guard.defuse();
        }
        // The rest (messages, tool Values, closures) drops here; `Drop` sees both
        // guards already taken.
    }
    // The stream-run slab is owned by the same tasks (an :on-text agent round or a
    // standalone `llm/stream`) and leaks the same way on cancel — including the
    // DETACHED chat span each entry holds. The seam is single-slot, so this one
    // callback sweeps both slabs. Detached spans end without touching the
    // installed (canceller's) span stack, so a plain end is safe here.
    let stream_reaped: Vec<StreamRunState> = STREAM_RUNS.with(|r| {
        let mut slab = r.borrow_mut();
        let tokens: Vec<u64> = slab
            .iter()
            .filter(|(_, st)| st.owning_task_id == Some(task_id))
            .map(|(k, _)| *k)
            .collect();
        tokens.into_iter().filter_map(|t| slab.remove(&t)).collect()
    });
    for mut st in stream_reaped {
        if let Some(span) = st.span.take() {
            span.record_error("cancelled", "stream cancelled");
        }
        // The wire worker (if still running) streams into a dead channel and
        // releases its admission permit when the provider stream ends —
        // documented best-effort for the sync stream stage.
    }
}

/// Extract the integer handle token from a `__agent-*` native's args.
pub(super) fn agent_token_arg(args: &[Value], who: &str) -> Result<u64, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity(who, "1", args.len()));
    }
    args[0]
        .as_int()
        .filter(|n| *n >= 0)
        .map(|n| n as u64)
        .ok_or_else(|| SemaError::type_error("agent-run-handle", args[0].type_name()))
}

/// `__agent-begin(agent, input, opts-or-absent) → token-int`. Ports `__agent-run-blocking`'s
/// setup: session/memory seed, conversation-id resolution, message assembly, tool
/// schemas, system, telemetry, and the attached agent span; stores it in the slab.
pub(super) fn agent_begin(args: &[Value]) -> Result<Value, SemaError> {
    if args.len() < 2 || args.len() > 3 {
        return Err(SemaError::arity("agent/run", "2-3", args.len()));
    }
    let agent = args[0]
        .as_agent_rc()
        .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
    let user_msg = args[1]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| args[1].to_string());

    let opts = args.get(2).and_then(|v| v.as_map_rc());
    let has_opts = opts.is_some();

    let on_tool_call = opts
        .as_ref()
        .and_then(|o| o.get(&Value::keyword("on-tool-call")).cloned());
    let on_text = opts
        .as_ref()
        .and_then(|o| o.get(&Value::keyword("on-text")).cloned());
    let reasoning_effort = opts.as_ref().and_then(|o| o.opt_name("reasoning-effort"));

    // :session — seed history + conversation-id from a prior Conversation.
    let (session_messages, session_conv_id): (Vec<ChatMessage>, Option<String>) =
        if let Some(ref o) = opts {
            if let Some(sess_val) = o.get(&Value::keyword("session")) {
                if let Some(conv_rc) = sess_val.as_conversation_rc() {
                    let msgs: Vec<ChatMessage> = conv_rc
                        .messages
                        .iter()
                        .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
                        .collect();
                    let cid = conv_rc.metadata.get("conversation-id").cloned();
                    (msgs, cid)
                } else {
                    (Vec::new(), None)
                }
            } else {
                (Vec::new(), None)
            }
        } else {
            (Vec::new(), None)
        };

    // :memory — seed from the memory working set.
    let memory_handle: Option<Value> = opts
        .as_ref()
        .and_then(|o| o.get(&Value::keyword("memory")).cloned());
    let memory_seed: Vec<ChatMessage> = if let Some(ref h) = memory_handle {
        MEMORY_CALLBACKS.with(|c| {
            if let Some(ref cbs) = *c.borrow() {
                (cbs.get_working)(h).unwrap_or_default()
            } else {
                Vec::new()
            }
        })
    } else {
        Vec::new()
    };

    let output_conv_id: String = session_conv_id
        .clone()
        .or_else(|| {
            opts.as_ref()
                .and_then(|o| o.get(&Value::keyword("conversation-id")))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        })
        .unwrap_or_else(sema_otel::new_conversation_id);

    let conv_scope = ConvScope {
        conversation: Some(output_conv_id.clone()),
        session: opts
            .as_ref()
            .and_then(|o| o.get(&Value::keyword("session-id")))
            .and_then(|v| v.as_str().map(|s| s.to_string())),
        user: opts
            .as_ref()
            .and_then(|o| o.get(&Value::keyword("user-id")))
            .and_then(|v| v.as_str().map(|s| s.to_string())),
    };

    // Build messages: memory working set + session history + :messages history + new user.
    let mut messages: Vec<ChatMessage> = memory_seed;
    messages.extend(session_messages);
    if let Some(ref o) = opts {
        if let Some(history) = o.get(&Value::keyword("messages")) {
            let extra = sema_list_to_chat_messages(history)?;
            messages.extend(extra);
        }
    }
    let pre_user_count = messages.len();
    messages.push(ChatMessage::new("user", user_msg));

    let tool_schemas = build_tool_schemas(&agent.tools)?;
    let system = if agent.system.is_empty() {
        None
    } else {
        Some(agent.system.clone())
    };

    let first_input = messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.to_text())
        .unwrap_or_default();

    // Open conversation scope FIRST so the agent span carries the same ids, then start
    // the attached agent span (pushed onto the thread-local span stack; the per-task
    // otel swap preserves it across every park). Both guards live in the slab and are
    // dropped (balanced pop+end) in `__agent-finish` / `Drop`.
    let conv_guard = Some(sema_otel::set_conversation_scope(
        &output_conv_id,
        conv_scope.session.as_deref(),
        conv_scope.user.as_deref(),
    ));
    let agent_span = sema_otel::agent_span(Some(&agent.name));
    // User :tags / :metadata attached directly to the agent span (a `CallTelemetry`
    // guard cannot be held across the loop's suspensions; the runtime path attaches to the
    // agent root rather than threading CALL_TAGS through every round).
    if let Some(o) = opts.as_ref() {
        let tags = get_opt_string_list(o, "tags");
        let meta = get_opt_str_map(o, "metadata");
        if !tags.is_empty() {
            agent_span.set_tags(&tags);
        }
        if !meta.is_empty() {
            agent_span.set_metadata(&meta);
        }
    }

    let state = AgentLoopState {
        messages,
        tools: agent.tools.clone(),
        tool_schemas,
        model: agent.model.clone(),
        max_tokens: Some(4096),
        temperature: None,
        system,
        reasoning_effort,
        on_tool_call,
        on_text,
        round: 0,
        max_rounds: agent.max_turns,
        consecutive_errors: 0,
        pending_tool_calls: Vec::new(),
        last_content: String::new(),
        first_input,
        done: false,
        abort_error: None,
        final_pushed: false,
        output_conv_id,
        has_opts,
        memory_handle,
        pre_user_count,
        agent_model: agent.model.clone(),
        agent_span: Some(agent_span),
        conv_guard,
        usage_scope: Some(open_usage_scope()),
        owning_task_id: sema_core::current_task_id(),
    };

    let token = AGENT_RUN_NEXT_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    });
    AGENT_RUNS.with(|r| r.borrow_mut().insert(token, state));
    Ok(Value::int(token as i64))
}

/// `__chat-begin(messages, opts?) → token-int | nil`. The `:tools` twin of
/// `__agent-begin`: `llm/chat` takes the full message list + tools/model/system
/// inline per call — no defagent to unpack, no `:session`/`:memory` surface — so
/// this builds the SAME `AgentLoopState` shape directly from the parsed args,
/// mirroring `run_tool_loop`'s own setup (a caller-id-or-fresh conversation scope +
/// a nameless agent span) rather than `agent_begin`'s (which threads a defagent's
/// identity + `:session`/`:memory` resolution through). The options parsing below
/// matches `__llm-chat-blocking`.
///
/// Returns nil when no tool loop is needed — the same `tools.is_empty() ||
/// tool_mode == "none"` condition `__llm-chat-blocking` checks — so the prelude
/// dispatcher falls through to that native, which already offloads the
/// plain-completion case in async context (WP-LLM-SIMPLE); nothing agent-loop
/// specific (span, conversation scope, slab entry) is created on that path.
/// `has_opts: false` unconditionally, so `__agent-finish` returns llm/chat's
/// bare-string contract, never the `{:response ...}` agent envelope.
pub(super) fn chat_begin(args: &[Value]) -> Result<Value, SemaError> {
    if args.is_empty() || args.len() > 2 {
        return Err(SemaError::arity("llm/chat", "1-2", args.len()));
    }
    let messages = extract_messages(&args[0])?;

    let mut model = String::new();
    let mut max_tokens = None;
    let mut temperature = None;
    let mut system = None;
    let mut reasoning_effort = None;
    let mut tools: Vec<Value> = Vec::new();
    let mut tool_mode = "auto".to_string();
    let mut max_tool_rounds = 10usize;
    let mut on_tool_call: Option<Value> = None;
    let mut conv_scope = ConvScope::default();

    let opts = args.get(1).and_then(|v| v.as_map_rc());
    if let Some(ref o) = opts {
        conv_scope = ConvScope::from_opts(Some(o));
        model = o.opt_str("model").unwrap_or_default();
        max_tokens = o.opt_int("max-tokens").map(|n| n as u32);
        temperature = o.opt_f64("temperature");
        system = o.opt_str("system");
        reasoning_effort = o.opt_name("reasoning-effort");
        on_tool_call = o.get(&Value::keyword("on-tool-call")).cloned();
        if let Some(t) = o.get(&Value::keyword("tools")).and_then(|v| v.as_seq()) {
            tools = t.to_vec();
        }
        if let Some(mode) = o.get(&Value::keyword("tool-mode")) {
            if let Some(s) = mode.as_keyword() {
                tool_mode = s;
            }
        }
        if let Some(rounds) = o.get(&Value::keyword("max-tool-rounds")) {
            if let Some(n) = rounds.as_int() {
                max_tool_rounds = n as usize;
            }
        }
    }

    if tools.is_empty() || tool_mode == "none" {
        return Ok(Value::nil());
    }

    let tool_schemas = build_tool_schemas(&tools)?;
    let first_input = messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.to_text())
        .unwrap_or_default();

    // `run_tool_loop`'s own scope resolution (a caller-supplied id wins; otherwise
    // a fresh one) — not `agent_begin`'s :session/:memory-aware precedence, which
    // llm/chat has neither of.
    let output_conv_id = conv_scope
        .conversation
        .clone()
        .unwrap_or_else(sema_otel::new_conversation_id);
    let conv_guard = Some(sema_otel::set_conversation_scope(
        &output_conv_id,
        conv_scope.session.as_deref(),
        conv_scope.user.as_deref(),
    ));
    // Nameless agent span — matches `run_tool_loop`'s `agent_span(None)` call for
    // llm/chat (only `agent/run` names the span after its defagent).
    let agent_span = sema_otel::agent_span(None);
    if let Some(o) = opts.as_ref() {
        let tags = get_opt_string_list(o, "tags");
        let meta = get_opt_str_map(o, "metadata");
        if !tags.is_empty() {
            agent_span.set_tags(&tags);
        }
        if !meta.is_empty() {
            agent_span.set_metadata(&meta);
        }
    }

    let agent_model = model.clone();
    let state = AgentLoopState {
        messages,
        tools,
        tool_schemas,
        model,
        max_tokens,
        temperature,
        system,
        reasoning_effort,
        on_tool_call,
        on_text: None, // llm/chat doesn't stream
        round: 0,
        max_rounds: max_tool_rounds,
        consecutive_errors: 0,
        pending_tool_calls: Vec::new(),
        last_content: String::new(),
        first_input,
        done: false,
        abort_error: None,
        final_pushed: false,
        output_conv_id,
        has_opts: false, // llm/chat always returns the bare completion string
        memory_handle: None,
        pre_user_count: 0,
        agent_model,
        agent_span: Some(agent_span),
        conv_guard,
        // llm/chat always returns the bare string (has_opts: false above) — no map, so
        // no :usage to expose; don't open a scope nobody will read.
        usage_scope: None,
        owning_task_id: sema_core::current_task_id(),
    };

    let token = AGENT_RUN_NEXT_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    });
    AGENT_RUNS.with(|r| r.borrow_mut().insert(token, state));
    Ok(Value::int(token as i64))
}

/// Apply one provider round's response to the loop state and return the driver's
/// `{:done bool :has-tools bool}` map. Runs on the VM thread, either from the
/// runtime decoder or inline for the synchronous fallback. Short-borrows the slab.
pub(super) fn agent_apply_step_response(
    token: u64,
    resp: ChatResponse,
) -> Result<Value, SemaError> {
    AGENT_RUNS.with(|r| {
        let mut slab = r.borrow_mut();
        let st = slab
            .get_mut(&token)
            .ok_or_else(|| SemaError::Llm("agent-run handle not found".to_string()))?;
        st.last_content = resp.content.clone();
        let has_tools = !resp.tool_calls.is_empty();
        if has_tools {
            // Echo the assistant turn carrying tool_calls BEFORE the tool results, so
            // every provider can correlate them (OpenAI rejects orphan tool results).
            st.messages.push(ChatMessage::assistant_with_tool_calls(
                resp.content.clone(),
                resp.tool_calls.clone(),
            ));
            st.pending_tool_calls = resp.tool_calls;
            st.round += 1;
            // Round cap: mark done, but `:has-tools` stays true so the driver still runs
            // this round's tools (`__agent-exec-tools`) before finishing — matching
            // `run_tool_loop`, which executes the final round's tools and so leaves a
            // valid `assistant(tool_calls) → tool_result` history rather than a dangling
            // tool-call turn that a follow-up run would feed back and providers reject.
            if st.round >= st.max_rounds {
                st.done = true;
            }
        } else {
            // No tool calls → final turn; `__agent-finish` appends the plain assistant.
            st.done = true;
        }
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("done"), Value::bool(st.done));
        map.insert(Value::keyword("has-tools"), Value::bool(has_tools));
        Ok(Value::map(map))
    })
}

/// `__agent-step(token) → {:done bool :has-tools bool}`. One provider round: in a
/// runtime task it offloads and suspends on an External wait; the decoder uses the
/// finalize closure to build the result map. Otherwise it runs `do_complete`
/// synchronously. If the loop is already done (round cap or consecutive-error
/// abort set by `__agent-exec-tools`), returns immediately without a provider call.
pub(super) fn agent_step(ctx: &EvalContext, token: u64) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::NativeOutcome;

    // Short-borrow: bail out if the loop is already done (round cap / consec-error
    // abort), else build the request + snapshot on_text; then drop the borrow.
    enum StepPrep {
        Done,
        Run(Box<ChatRequest>, Option<Value>),
    }
    let prep = AGENT_RUNS.with(|r| {
        let slab = r.borrow();
        let st = slab
            .get(&token)
            .ok_or_else(|| SemaError::Llm("agent-run handle not found".to_string()))?;
        if st.done {
            return Ok(StepPrep::Done);
        }
        let mut request = ChatRequest::new(st.model.clone(), st.messages.clone());
        request.max_tokens = st.max_tokens.or(Some(4096));
        request.temperature = st.temperature;
        request.system = st.system.clone();
        request.reasoning_effort = st.reasoning_effort.clone();
        request.tools = st.tool_schemas.clone();
        Ok::<_, SemaError>(StepPrep::Run(Box::new(request), st.on_text.clone()))
    })?;

    let (request, on_text) = match prep {
        StepPrep::Done => {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("done"), Value::bool(true));
            map.insert(Value::keyword("has-tools"), Value::bool(false));
            return Ok(NativeOutcome::Return(Value::map(map)));
        }
        StepPrep::Run(req, on_text) => (*req, on_text),
    };

    // In a runtime quantum, a streaming (`:on-text`)
    // round opens a non-blocking stream run and hands the driver
    // `{:stream tok :on-text cb}` — the prelude drives `__stream-drive` in TASK
    // context (so the callback may itself suspend, and siblings interleave between
    // delta batches), then applies the assembled response via
    // `__agent-stream-apply`, feeding `agent_apply_step_response` unchanged. A plain
    // round offloads the provider call and suspends the active task on an External
    // wait, so two spawned `agent/run`s overlap across rounds and
    // `async/cancel` cuts the loop at an inter-round park.
    #[cfg(not(target_arch = "wasm32"))]
    if sema_core::in_runtime_quantum() {
        if let Some(cb) = on_text {
            // Mirror `do_complete_streaming`'s scope/span setup, detached (the
            // span is finalized by the stream decoder after the last park).
            let _conv = (sema_otel::current_conversation_id().is_none()).then(|| {
                sema_otel::set_conversation_scope(&sema_otel::new_conversation_id(), None, None)
            });
            let span = sema_otel::llm_span_detached("chat");
            span.set_request(
                request.temperature,
                request.max_tokens,
                &request.stop_sequences,
                request.reasoning_effort.as_deref(),
            );
            span.set_output_type(request.json_mode);
            let stream_token = stream_run_begin(request, span)?;
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("stream"), stream_token);
            map.insert(Value::keyword("on-text"), cb);
            return Ok(NativeOutcome::Return(Value::map(map)));
        }
        return do_complete_runtime_suspend(
            request,
            CompleteFinalize::new(move |resp| agent_apply_step_response(token, resp)),
        );
    }

    // Synchronous round: an `:on-text` streaming round drives the SSE stream inline on
    // the VM thread; a plain round in non-async context is the ordinary blocking
    // completion. Either way the usage is accounted once and the state updated inline.
    let response = match on_text.as_ref() {
        Some(cb) => do_complete_streaming(ctx, request, cb)?,
        None => do_complete(request)?,
    };
    track_usage(&response.usage)?;
    agent_apply_step_response(token, response).map(NativeOutcome::Return)
}

/// `__agent-exec-tools(token) → nil`. Runs the pending tool calls in ordinary runtime
/// task context (so async tools suspend correctly), pushing correlated
/// tool-result messages. Never holds the slab borrow across a callback / tool call.
pub(super) fn agent_exec_tools(ctx: &EvalContext, token: u64) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::NativeOutcome;
    // Short-borrow: copy out the pending calls + tool set + callback, then drop.
    let (pending, tools, on_tool_call): (Vec<ToolCall>, Vec<Value>, Option<Value>) = AGENT_RUNS
        .with(|r| {
            let mut slab = r.borrow_mut();
            let st = slab
                .get_mut(&token)
                .ok_or_else(|| SemaError::Llm("agent-run handle not found".to_string()))?;
            let pending = std::mem::take(&mut st.pending_tool_calls);
            Ok::<_, SemaError>((pending, st.tools.clone(), st.on_tool_call.clone()))
        })?;
    let denied = preflight_tool_calls(&pending, &tools)?;

    // Cooperative runtime path (Task 04/06): a tool handler may SUSPEND (e.g.
    // `mcp/call`'s runtime external wait, or an `async/await` inside the handler),
    // and the `:on-tool-call` callback may itself run a runtime op (the ticker test
    // sends on a channel from it). Drive each handler and callback as a
    // `NativeOutcome::Call` so they run as real cooperative work on the active task
    // and park/resume through the scheduler. The multi-round loop above
    // (`__agent-drive`) already runs turn-by-turn in bytecode; this makes the
    // per-turn tool round cooperative too. `ExecToolsContinuation` journals the same
    // per-tool OTel span + `:on-tool-call` start/end events + correlated tool
    // results (with the same error-recovery) the synchronous `run_tool_loop` does.
    if sema_core::in_runtime_quantum() {
        return exec_tools_cooperative_start(token, tools, on_tool_call, pending, denied);
    }

    for tc in &pending {
        if let Some(error) = denied.get(&tc.id) {
            record_tool_result(token, tc, error.clone(), true);
            continue;
        }
        let args_value = sema_core::json_to_value(&tc.arguments);

        if let Some(callback) = on_tool_call.as_ref() {
            let mut event_map = BTreeMap::new();
            event_map.insert(Value::keyword("event"), Value::string("start"));
            event_map.insert(Value::keyword("tool"), Value::string(&tc.name));
            event_map.insert(Value::keyword("args"), args_value.clone());
            let _ = sema_core::call_callback(ctx, callback, &[Value::map(event_map)]);
        }

        let start_time = std::time::Instant::now();
        let tool_desc = tools.iter().find_map(|t| {
            let td = t.as_tool_def_rc()?;
            (td.name == tc.name).then(|| td.description.clone())
        });
        let tspan = sema_otel::tool_span(&tc.name, &tc.id, tool_desc.as_deref());
        let (result, is_error) = match execute_tool_call(ctx, &tools, &tc.name, &tc.arguments) {
            Ok(r) => (r, false),
            Err(e) => (format!("Error: {e}"), true),
        };
        if is_error {
            tspan.record_error("tool_error", &result);
        }
        if sema_otel::content_capture_enabled() {
            let args_json = serde_json::to_string(&tc.arguments).unwrap_or_default();
            tspan.set_tool_io(&args_json, &result);
        }
        drop(tspan);
        let duration_ms = start_time.elapsed().as_millis() as i64;

        if let Some(callback) = on_tool_call.as_ref() {
            let mut event_map = BTreeMap::new();
            event_map.insert(Value::keyword("event"), Value::string("end"));
            event_map.insert(Value::keyword("tool"), Value::string(&tc.name));
            event_map.insert(Value::keyword("args"), args_value);
            let result_preview = if result.len() > 200 {
                format!("{}...", sema_core::truncate_chars(&result, 200))
            } else {
                result.clone()
            };
            event_map.insert(Value::keyword("result"), Value::string(&result_preview));
            event_map.insert(Value::keyword("error"), Value::bool(is_error));
            event_map.insert(Value::keyword("duration-ms"), Value::int(duration_ms));
            let _ = sema_core::call_callback(ctx, callback, &[Value::map(event_map)]);
        }

        // Re-borrow to push the correlated result + update the error counter.
        record_tool_result(token, tc, result, is_error);
    }

    Ok(NativeOutcome::Return(Value::nil()))
}

/// Push a correlated `tool_result` message for `tc` into the agent slab and
/// update the consecutive-error counter (aborting the loop past the cap, so
/// `__agent-finish` raises the same failure the blocking path returns). Shared by
/// the synchronous tool loop and the cooperative runtime continuation so both keep
/// identical loop-state semantics.
pub(super) fn record_tool_result(token: u64, tc: &ToolCall, result: String, is_error: bool) {
    AGENT_RUNS.with(|r| {
        let mut slab = r.borrow_mut();
        let st = match slab.get_mut(&token) {
            Some(st) => st,
            None => return,
        };
        st.messages.push(ChatMessage::tool_result(
            tc.id.clone(),
            tc.name.clone(),
            result,
        ));
        if is_error {
            st.consecutive_errors += 1;
            if st.consecutive_errors >= MAX_CONSECUTIVE_TOOL_ERRORS {
                st.done = true;
                st.abort_error = Some(format!(
                    "aborting agent run after {} consecutive tool errors",
                    st.consecutive_errors
                ));
            }
        } else {
            st.consecutive_errors = 0;
        }
    });
}

/// Which cooperative `Call` the next [`ExecToolsContinuation::resume`] is settling.
pub(super) enum ToolPhase {
    /// Resuming from a custom tool-schema predicate. Validation failures are
    /// accumulated after the start observer and before handler dispatch.
    Validation {
        key_name: String,
        failure_message: String,
    },
    /// Resuming from the `:on-tool-call` "start" event callback: its return value is
    /// ignored; next, open the tool span and validate the call.
    StartEvent,
    /// Resuming from the tool handler itself: its `Returned`/`Failed` becomes the
    /// tool result fed back to the model.
    Handler,
    /// Resuming from the `:on-tool-call` "end" event callback: its return is ignored;
    /// record the (already-computed) result and advance to the next tool.
    EndEvent { result: String, is_error: bool },
}

/// Per-call working state carried across the start-event, validation, handler, and
/// end-event calls while one tool call is in flight. The OTel span covers validation
/// and handler execution, matching the synchronous tool-call boundary.
pub(super) struct ActiveCall {
    pub(super) tc: ToolCall,
    /// `(handler, args)` resolved before validation, taken when the handler call
    /// is dispatched.
    pub(super) pending_handler: Option<(Value, Vec<Value>)>,
    pub(super) pending_error: Option<String>,
    pub(super) validation_steps: VecDeque<ExtractionValidationStep>,
    pub(super) validation_errors: Vec<String>,
    /// The call arguments as a Sema value (passed to both `:on-tool-call` events).
    pub(super) args_value: Value,
    /// The call arguments as JSON (for the tool span's content-gated I/O).
    pub(super) args_json: String,
    pub(super) span: Option<sema_otel::ToolSpan>,
    pub(super) started: Option<std::time::Instant>,
}

/// Cooperative tool-round state machine (Task 04/06). Drives each pending tool call's
/// `:on-tool-call` "start" event, its handler, and its "end" event as
/// `NativeOutcome::Call`s so a handler OR a callback that suspends parks/resumes on
/// the active runtime task; opens the same per-tool OTel span and records each
/// correlated result via [`record_tool_result`]. Resolution/validation failures and
/// handler errors are fed back as tool-error results (never escaping the loop),
/// mirroring `run_tool_loop`. `Return(nil)` once every pending call is recorded.
pub(super) struct ExecToolsContinuation {
    pub(super) token: u64,
    pub(super) tools: Vec<Value>,
    /// The `:on-tool-call` callback (workflow journaling / user observer), or `None`.
    pub(super) on_tool_call: Option<Value>,
    /// Tool calls not yet dispatched (front = next).
    pub(super) remaining: std::collections::VecDeque<ToolCall>,
    pub(super) denied: BTreeMap<String, String>,
    /// The call currently in flight, plus which `Call` the next `resume` settles.
    pub(super) active: Option<ActiveCall>,
    pub(super) phase: ToolPhase,
}

impl sema_core::runtime::Trace for ExecToolsContinuation {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        use sema_core::cycle::GcEdge;
        for tool in &self.tools {
            sink(GcEdge::Value(tool));
        }
        if let Some(cb) = &self.on_tool_call {
            sink(GcEdge::Value(cb));
        }
        if let Some(active) = &self.active {
            sink(GcEdge::Value(&active.args_value));
            if let Some((handler, args)) = &active.pending_handler {
                sink(GcEdge::Value(handler));
                for arg in args {
                    sink(GcEdge::Value(arg));
                }
            }
            for step in &active.validation_steps {
                if let ExtractionValidationStep::Predicate {
                    callable, argument, ..
                } = step
                {
                    sink(GcEdge::Value(callable));
                    sink(GcEdge::Value(argument));
                }
            }
        }
        true
    }
}

/// Build one `:on-tool-call` event map. `result` (present for the "end" event)
/// carries the truncated result preview, the error flag, and the handler duration.
pub(super) fn tool_event_map(
    event: &str,
    tc: &ToolCall,
    args_value: &Value,
    result: Option<(&str, bool, i64)>,
) -> Value {
    let mut m = BTreeMap::new();
    m.insert(Value::keyword("event"), Value::string(event));
    m.insert(Value::keyword("tool"), Value::string(&tc.name));
    m.insert(Value::keyword("args"), args_value.clone());
    if let Some((result_str, is_error, duration_ms)) = result {
        let preview = if result_str.len() > 200 {
            format!("{}...", sema_core::truncate_chars(result_str, 200))
        } else {
            result_str.to_string()
        };
        m.insert(Value::keyword("result"), Value::string(&preview));
        m.insert(Value::keyword("error"), Value::bool(is_error));
        m.insert(Value::keyword("duration-ms"), Value::int(duration_ms));
    }
    Value::map(m)
}

impl ExecToolsContinuation {
    fn start_tool_call(self: Box<Self>) -> sema_core::runtime::NativeResult {
        let start_event = self.on_tool_call.as_ref().map(|_| {
            let active = self.active.as_ref().expect("active tool call");
            tool_event_map("start", &active.tc, &active.args_value, None)
        });
        match self.on_tool_call.clone() {
            Some(callback) => {
                self.call_event(callback, start_event.unwrap(), ToolPhase::StartEvent)
            }
            None => self.begin_tool_work(),
        }
    }

    fn begin_tool_work(mut self: Box<Self>) -> sema_core::runtime::NativeResult {
        let active = self.active.as_mut().expect("active tool call");
        let tool_desc = self.tools.iter().find_map(|tool| {
            let definition = tool.as_tool_def_rc()?;
            (definition.name == active.tc.name).then(|| definition.description.clone())
        });
        active.span = Some(sema_otel::tool_span_detached(
            &active.tc.name,
            &active.tc.id,
            tool_desc.as_deref(),
        ));
        active.started = Some(std::time::Instant::now());

        if let Some(error) = active.pending_error.take() {
            return self.complete_tool_call(error, true);
        }
        self.advance_validation()
    }

    fn advance_validation(mut self: Box<Self>) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeCall, NativeOutcome};

        loop {
            let step = self
                .active
                .as_mut()
                .expect("active call while validating")
                .validation_steps
                .pop_front();
            match step {
                Some(ExtractionValidationStep::Error(error)) => self
                    .active
                    .as_mut()
                    .expect("active call while validating")
                    .validation_errors
                    .push(error),
                Some(ExtractionValidationStep::Predicate {
                    callable,
                    argument,
                    key_name,
                    failure_message,
                }) => {
                    self.phase = ToolPhase::Validation {
                        key_name,
                        failure_message,
                    };
                    return Ok(NativeOutcome::Call(NativeCall {
                        callable,
                        args: vec![argument],
                        continuation: self,
                    }));
                }
                None => break,
            }
        }

        let errors = &self
            .active
            .as_ref()
            .expect("active call after validation")
            .validation_errors;
        if errors.is_empty() {
            return self.dispatch_handler();
        }

        let active = self.active.as_ref().expect("invalid active call");
        let error = SemaError::Llm(format!(
            "invalid arguments for tool '{}': {}",
            active.tc.name,
            active.validation_errors.join("; ")
        ));
        self.complete_tool_call(format!("Error: {error}"), true)
    }

    /// A cooperative `Call` on `callback` with one event map, resuming into `phase`.
    fn call_event(
        mut self: Box<Self>,
        callback: Value,
        event: Value,
        phase: ToolPhase,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeCall, NativeOutcome};
        self.phase = phase;
        Ok(NativeOutcome::Call(NativeCall {
            callable: callback,
            args: vec![event],
            continuation: self,
        }))
    }

    /// Dispatch the validated handler as a `NativeOutcome::Call`, resuming into
    /// `Handler`. The tool span was opened before validation.
    fn dispatch_handler(mut self: Box<Self>) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeCall, NativeOutcome};
        let active = self.active.as_mut().expect("active call while dispatching");
        let (handler, args) = active
            .pending_handler
            .take()
            .expect("handler resolved before dispatch");
        self.phase = ToolPhase::Handler;
        Ok(NativeOutcome::Call(NativeCall {
            callable: handler,
            args,
            continuation: self,
        }))
    }

    /// Pop the next pending call and fire its start event. The no-observer path opens
    /// the span and begins validation directly. Returns `Return(nil)` once every
    /// pending call is recorded.
    fn advance(mut self: Box<Self>) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::NativeOutcome;
        let Some(tc) = self.remaining.pop_front() else {
            return Ok(NativeOutcome::Return(Value::nil()));
        };
        if let Some(error) = self.denied.remove(&tc.id) {
            record_tool_result(self.token, &tc, error, true);
            return self.advance();
        }
        let args_value = sema_core::json_to_value(&tc.arguments);
        let prepared = prepare_tool_call_cooperative(&self.tools, &tc.name, &tc.arguments);
        let (pending_handler, pending_error, validation_steps) = match prepared {
            Ok(prepared) => (
                Some((prepared.handler, prepared.args)),
                None,
                prepared.validation_steps,
            ),
            Err(error) => (None, Some(format!("Error: {error}")), VecDeque::new()),
        };
        let args_json = serde_json::to_string(&tc.arguments).unwrap_or_default();
        self.active = Some(ActiveCall {
            tc,
            pending_handler,
            pending_error,
            validation_steps,
            validation_errors: Vec::new(),
            args_value,
            args_json,
            span: None,
            started: None,
        });
        self.start_tool_call()
    }

    /// Finalize the tool span for the settled handler (record error / content I/O,
    /// then drop it) and return `(result, is_error, duration_ms)`.
    fn finish_span(&mut self, result: &str, is_error: bool) -> i64 {
        let active = self.active.as_mut().expect("active call at handler settle");
        if let Some(span) = active.span.take() {
            if is_error {
                span.record_error("tool_error", result);
            }
            if sema_otel::content_capture_enabled() {
                span.set_tool_io(&active.args_json, result);
            }
            // `span` drops here → the tool span ends and pops the task's otel stack.
        }
        active
            .started
            .map(|t| t.elapsed().as_millis() as i64)
            .unwrap_or(0)
    }

    fn complete_tool_call(
        mut self: Box<Self>,
        result: String,
        is_error: bool,
    ) -> sema_core::runtime::NativeResult {
        let duration_ms = self.finish_span(&result, is_error);
        match self.on_tool_call.clone() {
            Some(callback) => {
                let active = self.active.as_ref().expect("active call at completion");
                let event = tool_event_map(
                    "end",
                    &active.tc,
                    &active.args_value,
                    Some((&result, is_error, duration_ms)),
                );
                self.call_event(callback, event, ToolPhase::EndEvent { result, is_error })
            }
            None => {
                let tc = self.active.take().expect("active call at completion").tc;
                record_tool_result(self.token, &tc, result, is_error);
                self.advance()
            }
        }
    }
}

impl sema_core::runtime::NativeContinuation for ExecToolsContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;
        // A task cancellation lands on whichever `Call` was in flight; abort the whole
        // run so the runtime settles the parked parent task Cancelled.
        if let ResumeInput::Cancelled(reason) = &input {
            let message = format!("agent tool round was cancelled ({reason:?})");
            if let Some(span) = self.active.as_ref().and_then(|active| active.span.as_ref()) {
                span.record_error("cancelled", &message);
            }
            return Err(SemaError::eval(message));
        }
        match std::mem::replace(&mut self.phase, ToolPhase::Handler) {
            ToolPhase::Validation {
                key_name,
                failure_message,
            } => {
                let active = self.active.as_mut().expect("active call after validation");
                match input {
                    ResumeInput::Returned(value) if value.is_truthy() => {}
                    ResumeInput::Returned(_) => active
                        .validation_errors
                        .push(format!("key {key_name}: {failure_message}")),
                    ResumeInput::Failed(error) => active
                        .validation_errors
                        .push(format!("key {key_name}: validation error: {error}")),
                    ResumeInput::Cancelled(_) => unreachable!("handled above"),
                    ResumeInput::Runtime(_) => {
                        return Err(SemaError::eval(
                            "agent tool validation received an unexpected runtime response",
                        ))
                    }
                }
                self.advance_validation()
            }
            // The "start" event settled (return value ignored, and a callback failure
            // must not abort the tool round) — now open the span and validate.
            ToolPhase::StartEvent => self.begin_tool_work(),
            // The handler settled: its value/error is the tool result.
            ToolPhase::Handler => {
                let (result, is_error) = match input {
                    ResumeInput::Returned(value) => (stringify_tool_result(value), false),
                    // A handler error is fed BACK to the model as a tool result (the
                    // loop recovers), never propagated — matching `run_tool_loop`.
                    ResumeInput::Failed(error) => (format!("Error: {error}"), true),
                    ResumeInput::Cancelled(_) => unreachable!("handled above"),
                    ResumeInput::Runtime(_) => {
                        return Err(SemaError::eval(
                            "agent tool continuation received an unexpected runtime response",
                        ))
                    }
                };
                self.complete_tool_call(result, is_error)
            }
            // The "end" event settled — record the tool result and move on.
            ToolPhase::EndEvent { result, is_error } => {
                let tc = self.active.take().expect("active call at end").tc;
                record_tool_result(self.token, &tc, result, is_error);
                self.advance()
            }
        }
    }
}

/// Begin the cooperative tool round: build the [`ExecToolsContinuation`] and
/// RETURN its first `NativeOutcome` on the runtime native ABI (mirrors `map`'s
/// cooperative entry — the runtime drives the resulting `Call`). When no handler
/// call is needed (e.g. every pending call fails to resolve), the results are
/// recorded synchronously and a nil value is returned directly.
pub(super) fn exec_tools_cooperative_start(
    token: u64,
    tools: Vec<Value>,
    on_tool_call: Option<Value>,
    pending: Vec<ToolCall>,
    denied: BTreeMap<String, String>,
) -> sema_core::runtime::NativeResult {
    let continuation = Box::new(ExecToolsContinuation {
        token,
        tools,
        on_tool_call,
        remaining: pending.into(),
        denied,
        active: None,
        phase: ToolPhase::Handler,
    });
    continuation.advance()
}

/// `__agent-finish(token) → result`. Idempotent: appends the final assistant turn,
/// records trace I/O, ends the agent span, writes back to memory, and builds the
/// return value (`{:response :messages :session}` map with opts, else the string).
pub(super) fn agent_finish(token: u64, finish_error: Option<String>) -> Result<Value, SemaError> {
    // Take the state OUT of the slab so the span/scope guards drop (balanced pop+end,
    // agent-task otel installed) once we're done building the result.
    let mut st = match AGENT_RUNS.with(|r| r.borrow_mut().remove(&token)) {
        Some(st) => st,
        // Already finished (idempotent) — the driver's normal exit and the Sema catch
        // may both call finish.
        None => return Ok(Value::nil()),
    };

    // The driver's `catch` passed the unwinding error: close the `invoke_agent`
    // span with the failure status before it ends (on `st` drop below). This is
    // the cancellation path — the unified runtime resumes a cancelled parked task
    // so the driver's `catch → __agent-finish → throw` runs, ending the span here
    // (balanced pop) rather than leaving it for the task-reaped sweep. Without
    // this the span would end "unset", losing the cancellation telemetry.
    if let Some(err) = &finish_error {
        if let Some(span) = st.agent_span.as_ref() {
            let kind = if err.to_ascii_lowercase().contains("cancel") {
                "cancelled"
            } else {
                "agent_error"
            };
            span.record_error(kind, err);
        }
    }

    // Append the final assistant message (mirrors run_tool_loop's terminal push).
    if !st.final_pushed && !st.last_content.is_empty() {
        st.messages
            .push(ChatMessage::new("assistant", st.last_content.clone()));
        st.final_pushed = true;
    }
    if let Some(span) = st.agent_span.as_ref() {
        span.set_trace_io(&st.first_input, &st.last_content);
    }

    // Memory writeback: append new turns (from pre_user_count) into the memory thread.
    if let Some(ref h) = st.memory_handle {
        let new_turns = if st.messages.len() > st.pre_user_count {
            &st.messages[st.pre_user_count..]
        } else {
            &[]
        };
        MEMORY_CALLBACKS.with(|c| {
            if let Some(ref cbs) = *c.borrow() {
                let _ = (cbs.append_back)(h, new_turns);
            }
        });
    }

    // A consecutive-tool-error abort surfaces as an error (matching the blocking path).
    if let Some(msg) = st.abort_error.take() {
        if let Some(span) = st.agent_span.as_ref() {
            span.record_error("tool_error", &msg);
        }
        // `st` drops here → agent span ends, conv scope restored.
        return Err(SemaError::Llm(msg));
    }

    let result = st.last_content.clone();
    if st.has_opts {
        let mut meta = std::collections::BTreeMap::new();
        meta.insert("conversation-id".to_string(), st.output_conv_id.clone());
        let session_conv = Conversation {
            messages: st
                .messages
                .iter()
                .map(|m| Message {
                    role: match m.role.as_str() {
                        "assistant" => Role::Assistant,
                        _ => Role::User,
                    },
                    content: m.content.to_text(),
                    images: Vec::new(),
                })
                .collect(),
            model: st.agent_model.clone(),
            metadata: meta,
        };
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("response"), Value::string(&result));
        map.insert(
            Value::keyword("messages"),
            chat_messages_to_sema_list(&st.messages),
        );
        map.insert(Value::keyword("session"), Value::conversation(session_conv));
        let usage = st
            .usage_scope
            .as_ref()
            .map(|s| s.usage())
            .unwrap_or_default();
        map.insert(Value::keyword("usage"), agent_usage_value(&usage));
        Ok(Value::map(map))
    } else {
        Ok(Value::string(&result))
    }
    // `st` drops at the end of scope → agent span ends (balanced), conv scope restored.
}

// ── Non-blocking streaming (`llm/stream` + agent `:on-text` rounds) ──────────
//
// The streaming sibling of the `__agent-*` loop above, same ADR #68 shape: a
// native cannot retain a Rust loop across suspension, and a Sema callback cannot
// run inside a completion decoder, so the per-delta loop lives in bytecode
// (`__stream-drive` in the prelude) over three natives — `__stream-begin` /
// `__stream-next` / `__stream-finish` — coordinated by a slab entry that owns
// the wire channel and the finalize context. The wire side (the provider's
// synchronous SSE drive) runs on the I/O pool, sending each delta over an mpsc
// channel; only `String`s and the final `ChatResponse` cross the thread
// boundary, never a Sema `Value`.

/// `__agent-stream-apply(agent-token, stream-token) → {:done :has-tools}`. The
/// agent-path terminal for a driven streaming round: pops the stream slab entry
/// and feeds the assembled `ChatResponse` to `agent_apply_step_response`
/// unchanged (tool-call handling identical to a non-streaming round; usage was
/// accounted by the stream finalizer).
pub(super) fn agent_stream_apply(agent_token: u64, stream_token: u64) -> Result<Value, SemaError> {
    let mut st = STREAM_RUNS
        .with(|r| r.borrow_mut().remove(&stream_token))
        .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
    if let Some(error) = st.pending_error.take() {
        return Err(error);
    }
    let resp = st
        .response
        .take()
        .ok_or_else(|| SemaError::Llm("stream not finished".to_string()))?;
    agent_apply_step_response(agent_token, resp)
}

/// The tool execution loop: send -> check for tool_calls -> execute -> send results -> repeat.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_tool_loop(
    ctx: &EvalContext,
    initial_messages: Vec<ChatMessage>,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    system: Option<String>,
    reasoning_effort: Option<String>,
    tools: &[Value],
    tool_schemas: &[ToolSchema],
    max_rounds: usize,
    on_tool_call: Option<&Value>,
    on_text: Option<&Value>,
    agent_name: Option<&str>,
    ids: ConvScope,
) -> Result<(String, Vec<ChatMessage>), SemaError> {
    // Open the conversation/session/user scope FIRST so the agent span and every
    // nested chat/tool span carry the same gen_ai.conversation.id (+ session.id /
    // user.id). A caller-supplied id wins; otherwise generate a fresh one.
    let conv = ids
        .conversation
        .clone()
        .unwrap_or_else(sema_otel::new_conversation_id);
    let _conv_scope =
        sema_otel::set_conversation_scope(&conv, ids.session.as_deref(), ids.user.as_deref());
    // INTERNAL agent span over the whole loop; the per-round `chat` spans (from
    // do_complete) and per-tool spans nest under it via the thread-local stack.
    let _agent_span = sema_otel::agent_span(agent_name);
    // User :tags / :metadata for this run, attached to the agent root span.
    apply_call_telemetry_agent(&_agent_span);
    let mut messages = initial_messages;
    // First user input for the trace-level I/O rollup (compat: Langfuse trace panel).
    let first_input = messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.to_text())
        .unwrap_or_default();
    let mut last_content = String::new();
    // Bound runaway error loops: if the model keeps issuing failing tool calls
    // and never recovers, abort rather than burning every round. Reset on any
    // successful tool call.
    const MAX_CONSECUTIVE_TOOL_ERRORS: usize = 5;
    let mut consecutive_errors: usize = 0;

    for _round in 0..max_rounds {
        let mut request = ChatRequest::new(model.clone(), messages.clone());
        request.max_tokens = max_tokens.or(Some(4096));
        request.temperature = temperature;
        request.system = system.clone();
        request.reasoning_effort = reasoning_effort.clone();
        request.tools = tool_schemas.to_vec();

        // Stream the assistant text live when the caller supplied :on-text;
        // otherwise take the plain (cache-eligible) path. Tool-call handling and
        // usage accounting below are identical either way.
        let completion = match on_text {
            Some(cb) => do_complete_streaming(ctx, request, cb),
            None => do_complete(request),
        };
        let response = match completion {
            Ok(r) => r,
            Err(e) => {
                _agent_span.record_error("provider_error", &e.to_string());
                return Err(e);
            }
        };
        if let Err(e) = track_usage(&response.usage) {
            _agent_span.record_error("budget_error", &e.to_string());
            return Err(e);
        }
        last_content = response.content.clone();

        if response.tool_calls.is_empty() {
            // Push final assistant message onto history
            if !last_content.is_empty() {
                messages.push(ChatMessage::new("assistant", last_content.clone()));
            }
            _agent_span.set_trace_io(&first_input, &last_content);
            return Ok((last_content, messages));
        }
        // Gate the entire batch before any callback, validation predicate, or handler.
        // A hard denial aborts the round with zero tool side effects; `:tool-error`
        // denials become correlated tool results while allowed siblings may proceed.
        let denied = preflight_tool_calls(&response.tool_calls, tools)?;

        // Echo the assistant turn that invoked the tools, carrying the tool_calls
        // so the provider can correlate the tool results that follow. This MUST be
        // present (even with empty content) — OpenAI-family providers reject a
        // tool result that isn't preceded by the assistant tool_calls it answers.
        messages.push(ChatMessage::assistant_with_tool_calls(
            response.content.clone(),
            response.tool_calls.clone(),
        ));

        // Execute each tool call and add results
        for tc in &response.tool_calls {
            if let Some(error) = denied.get(&tc.id) {
                consecutive_errors += 1;
                messages.push(ChatMessage::tool_result(
                    tc.id.clone(),
                    tc.name.clone(),
                    error.clone(),
                ));
                if consecutive_errors >= MAX_CONSECUTIVE_TOOL_ERRORS {
                    let msg = format!(
                        "aborting agent run after {consecutive_errors} consecutive tool errors"
                    );
                    _agent_span.record_error("tool_error", &msg);
                    return Err(SemaError::Llm(msg));
                }
                continue;
            }
            // Build args map for callback
            let args_value = sema_core::json_to_value(&tc.arguments);

            // Fire "start" event
            if let Some(callback) = on_tool_call {
                let mut event_map = BTreeMap::new();
                event_map.insert(Value::keyword("event"), Value::string("start"));
                event_map.insert(Value::keyword("tool"), Value::string(&tc.name));
                event_map.insert(Value::keyword("args"), args_value.clone());
                let _ = sema_core::call_callback(ctx, callback, &[Value::map(event_map)]);
            }

            let start_time = std::time::Instant::now();
            // INTERNAL tool span (self-times over execute_tool_call, the one real
            // latency source). v1.41 requires the tool name in the span name.
            let tool_desc = tools.iter().find_map(|t| {
                let td = t.as_tool_def_rc()?;
                (td.name == tc.name).then(|| td.description.clone())
            });
            let tspan = sema_otel::tool_span(&tc.name, &tc.id, tool_desc.as_deref());
            // A failing or invalid tool call must NOT abort the whole agent run.
            // Capture the error as the tool result and feed it back so the model
            // can self-correct (bounded by MAX_CONSECUTIVE_TOOL_ERRORS / max_rounds).
            let (result, is_error) = match execute_tool_call(ctx, tools, &tc.name, &tc.arguments) {
                Ok(r) => {
                    consecutive_errors = 0;
                    (r, false)
                }
                Err(e) => {
                    consecutive_errors += 1;
                    (format!("Error: {e}"), true)
                }
            };
            if is_error {
                tspan.record_error("tool_error", &result);
            }
            // Tool args + result on the span (content-gated; canonical
            // gen_ai.tool.call.* + compat aliases) — the key agent-debugging datum.
            if sema_otel::content_capture_enabled() {
                let args_json = serde_json::to_string(&tc.arguments).unwrap_or_default();
                tspan.set_tool_io(&args_json, &result);
            }
            drop(tspan);
            let duration_ms = start_time.elapsed().as_millis() as i64;

            // Fire "end" event
            if let Some(callback) = on_tool_call {
                let mut event_map = BTreeMap::new();
                event_map.insert(Value::keyword("event"), Value::string("end"));
                event_map.insert(Value::keyword("tool"), Value::string(&tc.name));
                event_map.insert(Value::keyword("args"), args_value);
                // Truncate result for the callback to avoid huge payloads.
                // Use char-boundary truncation: a byte slice (`&result[..200]`)
                // panics when byte 200 lands inside a multi-byte character.
                let result_preview = if result.len() > 200 {
                    format!("{}...", sema_core::truncate_chars(&result, 200))
                } else {
                    result.clone()
                };
                event_map.insert(Value::keyword("result"), Value::string(&result_preview));
                event_map.insert(Value::keyword("error"), Value::bool(is_error));
                event_map.insert(Value::keyword("duration-ms"), Value::int(duration_ms));
                let _ = sema_core::call_callback(ctx, callback, &[Value::map(event_map)]);
            }

            // Correlated tool result — keyed by the call id and tool name — rather
            // than free-form user text, so every provider can match it to the call.
            messages.push(ChatMessage::tool_result(
                tc.id.clone(),
                tc.name.clone(),
                result,
            ));

            if consecutive_errors >= MAX_CONSECUTIVE_TOOL_ERRORS {
                let msg = format!(
                    "aborting agent run after {consecutive_errors} consecutive tool errors"
                );
                _agent_span.record_error("tool_error", &msg);
                return Err(SemaError::Llm(msg));
            }
        }

        // Agent-turn safe point (CORE-2, plan §5.2 point c): the round's tool
        // handlers just ran arbitrary Sema code (the long-running-agent leak
        // shape — recursive local helpers, channels, promises created per
        // turn), and a long agent run never returns to a top-level safe point
        // until it finishes. Threshold-gated. No pins: sema-llm cannot see the
        // executing VM's env (it depends only on sema-core), and pins are a
        // pure descent-skip optimization — correctness comes from external
        // strong counts. Message history/correlation is untouched.
        sema_core::gc_maybe_collect(&[], sema_core::GcTrigger::AgentTurn);
    }

    // Push final assistant message if we exhausted rounds
    if !last_content.is_empty() {
        messages.push(ChatMessage::new("assistant", last_content.clone()));
    }
    _agent_span.set_trace_io(&first_input, &last_content);
    Ok((last_content, messages))
}

/// Execute a tool call by finding the handler and invoking it.
pub(super) fn execute_tool_call(
    ctx: &EvalContext,
    tools: &[Value],
    name: &str,
    arguments: &serde_json::Value,
) -> Result<String, SemaError> {
    // Find the tool definition
    let tool_def = tools
        .iter()
        .find_map(|t| {
            let td = t.as_tool_def_rc()?;
            if td.name == name {
                Some(td)
            } else {
                None
            }
        })
        .ok_or_else(|| SemaError::Llm(format!("tool not found: {name}")))?;

    // Validate the model-supplied arguments against the tool's parameter schema
    // before invoking the handler, so a missing/wrong-typed argument is reported
    // back to the model (via the loop's error-recovery path) and it can retry with
    // corrected args — rather than silently calling the handler with bad input.
    // (Reuses the extraction validator; both schema and args use keyword keys.)
    let args_map = sema_core::json_to_value(arguments);
    if let Err(msg) = validate_extraction(&args_map, &tool_def.parameters) {
        return Err(SemaError::Llm(format!(
            "invalid arguments for tool '{name}': {msg}"
        )));
    }

    // Convert JSON arguments to Sema values and call the handler
    let sema_args = json_args_to_sema(&tool_def.parameters, arguments, &tool_def.handler);
    let result = sema_core::call_callback(ctx, &tool_def.handler, &sema_args)?;

    Ok(stringify_tool_result(result))
}

/// Convert a tool handler's return value to the string sent back to the model.
/// Strings pass through; maps/sequences are JSON-encoded; everything else uses
/// its display form. Shared by the synchronous `execute_tool_call` and the
/// cooperative runtime tool loop so both stringify identically.
pub(super) fn stringify_tool_result(result: Value) -> String {
    if let Some(s) = result.as_str() {
        return s.to_string();
    }
    if result.as_map_rc().is_some() || result.as_seq().is_some() {
        let json = sema_core::value_to_json_lossy(&result);
        serde_json::to_string(&json).unwrap_or_else(|_| result.to_string())
    } else {
        result.to_string()
    }
}

/// Drives `(tool/invoke tool args)`: runs the argument-validation steps (custom
/// `:validate` predicates dispatch as structural calls), then tail-calls the
/// handler and passes its raw return value through unchanged.
pub(super) struct ToolInvokeContinuation {
    tool_name: String,
    steps: VecDeque<ExtractionValidationStep>,
    errors: Vec<String>,
    /// `(key_name, failure_message)` for the predicate call in flight.
    pending_predicate: Option<(String, String)>,
    /// Taken at handler dispatch; a `Returned` with no pending predicate is the
    /// handler settling.
    handler: Option<(Value, Vec<Value>)>,
}

impl ToolInvokeContinuation {
    fn advance(mut self: Box<Self>) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeCall, NativeOutcome};
        while let Some(step) = self.steps.pop_front() {
            match step {
                ExtractionValidationStep::Error(error) => self.errors.push(error),
                ExtractionValidationStep::Predicate {
                    callable,
                    argument,
                    key_name,
                    failure_message,
                } => {
                    self.pending_predicate = Some((key_name, failure_message));
                    return Ok(NativeOutcome::Call(NativeCall {
                        callable,
                        args: vec![argument],
                        continuation: self,
                    }));
                }
            }
        }
        if !self.errors.is_empty() {
            return Err(SemaError::Llm(format!(
                "invalid arguments for tool '{}': {}",
                self.tool_name,
                self.errors.join("; ")
            )));
        }
        let (handler, args) = self.handler.take().expect("handler present until dispatch");
        Ok(NativeOutcome::Call(NativeCall {
            callable: handler,
            args,
            continuation: self,
        }))
    }
}

impl sema_core::runtime::Trace for ToolInvokeContinuation {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        for step in &self.steps {
            if let ExtractionValidationStep::Predicate {
                callable, argument, ..
            } = step
            {
                sink(sema_core::cycle::GcEdge::Value(callable));
                sink(sema_core::cycle::GcEdge::Value(argument));
            }
        }
        if let Some((handler, args)) = &self.handler {
            sink(sema_core::cycle::GcEdge::Value(handler));
            for value in args {
                sink(sema_core::cycle::GcEdge::Value(value));
            }
        }
        true
    }
}

impl sema_core::runtime::NativeContinuation for ToolInvokeContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeOutcome, ResumeInput};
        match input {
            ResumeInput::Returned(value) => match self.pending_predicate.take() {
                Some((key_name, failure_message)) => {
                    if !value.is_truthy() {
                        self.errors
                            .push(format!("key {key_name}: {failure_message}"));
                    }
                    self.advance()
                }
                None => Ok(NativeOutcome::Return(value)),
            },
            ResumeInput::Failed(error) => match self.pending_predicate.take() {
                Some((key_name, _)) => {
                    self.errors
                        .push(format!("key {key_name}: validation error: {error}"));
                    self.advance()
                }
                None => Err(error),
            },
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "tool/invoke was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "tool/invoke received an unexpected runtime response",
            )),
        }
    }
}

pub(super) struct CooperativeToolCall {
    handler: Value,
    args: Vec<Value>,
    validation_steps: VecDeque<ExtractionValidationStep>,
}

/// Resolve a runtime tool call without invoking custom schema predicates. The
/// caller drives each predicate through `NativeOutcome::Call` before dispatching
/// the handler.
pub(super) fn prepare_tool_call_cooperative(
    tools: &[Value],
    name: &str,
    arguments: &serde_json::Value,
) -> Result<CooperativeToolCall, SemaError> {
    let tool_def = tools
        .iter()
        .find_map(|tool| {
            tool.as_tool_def_rc()
                .filter(|definition| definition.name == name)
        })
        .ok_or_else(|| SemaError::Llm(format!("tool not found: {name}")))?;

    let args_map = sema_core::json_to_value(arguments);
    let validation_steps = prepare_extraction_validation(&args_map, &tool_def.parameters);
    let args = json_args_to_sema(&tool_def.parameters, arguments, &tool_def.handler);
    Ok(CooperativeToolCall {
        handler: tool_def.handler.clone(),
        args,
        validation_steps,
    })
}

/// Convert JSON arguments into a list of Sema values based on handler declaration order.
/// Falling back to the parameter map uses BTreeMap key order (alphabetical).
/// Look up a `:default` declared on a deftool param spec, e.g.
/// `{:name {:type "string" :default "world"}}`.
pub(super) fn param_default(params: &Value, key: &str) -> Option<Value> {
    let inner = params.as_map_rc()?.get(&Value::keyword(key))?.as_map_rc()?;
    inner.get(&Value::keyword("default")).cloned()
}

pub(super) fn json_args_to_sema(
    params: &Value,
    arguments: &serde_json::Value,
    handler: &Value,
) -> Vec<Value> {
    if let serde_json::Value::Object(json_obj) = arguments {
        if let Some(param_names) = handler_param_names(handler) {
            return param_names
                .iter()
                .map(|name| {
                    let key = resolve(*name);
                    json_obj
                        .get(&key)
                        .map(sema_core::json_to_value)
                        .unwrap_or_else(|| param_default(params, &key).unwrap_or(Value::nil()))
                })
                .collect();
        }
        // Fallback: use param map keys (BTreeMap order — alphabetical)
        if let Some(param_map) = params.as_map_rc() {
            return param_map
                .keys()
                .map(|k| {
                    let key_str = k
                        .as_keyword()
                        .or_else(|| k.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| k.to_string());
                    json_obj
                        .get(&key_str)
                        .map(sema_core::json_to_value)
                        .unwrap_or_else(|| param_default(params, &key_str).unwrap_or(Value::nil()))
                })
                .collect();
        }
    }
    vec![sema_core::json_to_value(arguments)]
}

pub(super) fn handler_param_names(handler: &Value) -> Option<std::rc::Rc<[sema_core::Spur]>> {
    if let Some(lambda) = handler.as_lambda_rc() {
        return Some(lambda.params.as_slice().into());
    }

    handler
        .as_native_fn_ref()
        .filter(|native| native.is_closure)
        .and_then(|native| native.param_names.clone())
}

pub(super) fn register(env: &Env, sandbox: &sema_core::Sandbox) {
    // (agent/run agent "msg") returns string
    // (agent/run agent "msg" {:on-tool-call cb :messages history}) returns {:response "..." :messages [...]}
    // Synchronous agent loop. The prelude dispatcher reaches this native outside a
    // runtime quantum and uses the suspend-per-round bytecode driver inside one.
    register_fn_ctx(env, "__agent-run-blocking", |ctx, args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("agent/run", "2-3", args.len()));
        }
        if sema_core::in_runtime_quantum() {
            return Err(SemaError::eval(
                "__agent-run-blocking cannot run inside the cooperative runtime",
            )
            .with_hint(
                "call agent/run so provider, observer, and tool callbacks can suspend cooperatively",
            ));
        }
        let agent = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        let user_msg = args[1]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[1].to_string());

        // Extract options from 3rd arg
        let opts = args.get(2).and_then(|v| v.as_map_rc());

        let on_tool_call = opts
            .as_ref()
            .and_then(|o| o.get(&Value::keyword("on-tool-call")).cloned());

        // Optional streaming hook: called with each assistant text delta so a TUI
        // can render the reply live. Absent → non-streaming (unchanged) behavior.
        let on_text = opts
            .as_ref()
            .and_then(|o| o.get(&Value::keyword("on-text")).cloned());

        // Optional per-run reasoning effort, e.g. (agent/run a msg {:reasoning-effort :high}).
        let reasoning_effort = opts.as_ref().and_then(|o| o.opt_name("reasoning-effort"));

        // ── Phase 1: :session input — seed history from a prior Conversation ──
        // When :session is a Conversation value, extract its messages as the initial
        // history (so turn 2 sees turn 1's full history). Also extract its
        // conversation-id so telemetry threads across turns.
        let (session_messages, session_conv_id): (Vec<ChatMessage>, Option<String>) =
            if let Some(ref o) = opts {
                if let Some(sess_val) = o.get(&Value::keyword("session")) {
                    if let Some(conv_rc) = sess_val.as_conversation_rc() {
                        let msgs: Vec<ChatMessage> = conv_rc
                            .messages
                            .iter()
                            .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
                            .collect();
                        let cid = conv_rc.metadata.get("conversation-id").cloned();
                        (msgs, cid)
                    } else {
                        (Vec::new(), None)
                    }
                } else {
                    (Vec::new(), None)
                }
            } else {
                (Vec::new(), None)
            };

        // ── :memory opt — seed from memory working set ────────────────────────
        // If :memory is given and memory callbacks are registered, extract the working
        // messages and prepend them before any :session messages. After the run, the
        // new turns are appended back into the memory.
        let memory_handle: Option<Value> = opts
            .as_ref()
            .and_then(|o| o.get(&Value::keyword("memory")).cloned());

        let memory_seed: Vec<ChatMessage> = if let Some(ref h) = memory_handle {
            MEMORY_CALLBACKS.with(|c| {
                if let Some(ref cbs) = *c.borrow() {
                    (cbs.get_working)(h).unwrap_or_default()
                } else {
                    Vec::new()
                }
            })
        } else {
            Vec::new()
        };
        let memory_seed_len = memory_seed.len();

        // Generate (or reuse) the conversation-id BEFORE run_tool_loop so we can
        // attach it to the :session output conversation. Explicit :conversation-id opt
        // wins; then the :session's stored id; otherwise generate a fresh one.
        let output_conv_id: String = session_conv_id
            .clone()
            .or_else(|| {
                opts.as_ref()
                    .and_then(|o| o.get(&Value::keyword("conversation-id")))
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_else(sema_otel::new_conversation_id);

        let conv_scope = ConvScope {
            conversation: Some(output_conv_id.clone()),
            session: opts
                .as_ref()
                .and_then(|o| o.get(&Value::keyword("session-id")))
                .and_then(|v| v.as_str().map(|s| s.to_string())),
            user: opts
                .as_ref()
                .and_then(|o| o.get(&Value::keyword("user-id")))
                .and_then(|v| v.as_str().map(|s| s.to_string())),
        };

        // Build messages: memory working set + session history + :messages history + new user.
        // Track the pre-user-push count so we can slice new turns for memory append.
        let mut messages: Vec<ChatMessage> = memory_seed;
        messages.extend(session_messages);
        if let Some(ref o) = opts {
            if let Some(history) = o.get(&Value::keyword("messages")) {
                let extra = sema_list_to_chat_messages(history)?;
                messages.extend(extra);
            }
        }
        // Capture the index of the first NEW turn (user+assistant) before the user push.
        let pre_user_count = messages.len();
        messages.push(ChatMessage::new("user", user_msg));

        let tool_schemas = build_tool_schemas(&agent.tools)?;
        let system = if agent.system.is_empty() {
            None
        } else {
            Some(agent.system.clone())
        };

        // Per-run observability tags/metadata: attached to the agent span (and inherited
        // by the nested per-round chat spans) inside run_tool_loop.
        let _tele = install_call_telemetry(opts.as_ref());

        // Tally this turn's usage independently of the process-global `llm/session-usage`
        // (issue #86). Nests correctly under an enclosing `workflow/step` scope, if any —
        // `UsageScope::Drop` folds this leaf's tally into the parent's before restoring it.
        let _usage_scope = open_usage_scope();

        let (result, final_messages) = run_tool_loop(
            ctx,
            messages,
            agent.model.clone(),
            Some(4096),
            None,
            system,
            reasoning_effort,
            &agent.tools,
            &tool_schemas,
            agent.max_turns,
            on_tool_call.as_ref(),
            on_text.as_ref(),
            Some(&agent.name),
            conv_scope,
        )?;

        // ── :memory post-run: append new turns back into the memory thread ────
        // Append turns from pre_user_count onward (user turn + new assistant turns).
        // This excludes the memory seed (already in memory) but includes session/extra
        // history only if it was new (which is correct — those are the new turns).
        // We want to persist user + assistant: slice from pre_user_count.
        if let Some(ref h) = memory_handle {
            let new_turns = if final_messages.len() > pre_user_count {
                &final_messages[pre_user_count..]
            } else {
                &[]
            };
            let _ = memory_seed_len; // consumed above, silence warning
            MEMORY_CALLBACKS.with(|c| {
                if let Some(ref cbs) = *c.borrow() {
                    let _ = (cbs.append_back)(h, new_turns);
                }
            });
        }

        // 3-arg form with opts: return {:response "..." :messages [...] :session <conv>}
        if opts.is_some() {
            let mut meta = std::collections::BTreeMap::new();
            meta.insert("conversation-id".to_string(), output_conv_id);
            let session_conv = Conversation {
                messages: final_messages
                    .iter()
                    .map(|m| Message {
                        role: match m.role.as_str() {
                            "assistant" => Role::Assistant,
                            _ => Role::User,
                        },
                        content: m.content.to_text(),
                        images: Vec::new(),
                    })
                    .collect(),
                model: agent.model.clone(),
                metadata: meta,
            };
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("response"), Value::string(&result));
            map.insert(
                Value::keyword("messages"),
                chat_messages_to_sema_list(&final_messages),
            );
            map.insert(Value::keyword("session"), Value::conversation(session_conv));
            map.insert(
                Value::keyword("usage"),
                agent_usage_value(&_usage_scope.usage()),
            );
            Ok(Value::map(map))
        } else {
            // 2-arg form: return string (backward compat)
            Ok(Value::string(&result))
        }
    });

    // ── Non-blocking multi-round agent loop (runtime-task path) ───────────────
    // The prelude `agent/run` dispatches here (four internal natives + a Sema
    // driver loop) when `(__async-context?)`, so each provider round suspends on
    // an External wait and sibling tasks overlap during the conversation.
    // See docs/plans/2026-07-02-nonblocking-agent-run.md (ADR #68).
    register_fn_ctx(env, "__async-context?", |_ctx, _args| {
        Ok(Value::bool(in_runtime_offload_context()))
    });
    // True while a unified-runtime VM quantum is executing — at ANY level, the
    // root/top-level main task INCLUDED (it too is a genuine runtime task that can
    // park and resume; it merely publishes `current_task_id() == None` because it
    // is not `async/cancel`-addressable). The prelude `agent/run` / `llm/chat`
    // dispatchers select the Sema-driven `__agent-drive` loop whenever this is true,
    // so each provider round offloads + suspends and the tool round
    // (`__agent-exec-tools`) runs every handler COOPERATIVELY via `NativeOutcome::Call`
    // — a handler that suspends (e.g. `mcp/call`'s runtime external wait, or an
    // `async/await` inside it) parks on the active task and resumes through the
    // scheduler, at the root exactly as in a spawned child. The cooperative tool
    // round journals the same per-tool OTel span + `:on-tool-call` start/end events
    // the synchronous `run_tool_loop` does (see `ExecToolsContinuation`), so the flip
    // is span/journaling-transparent. The synchronous `run_tool_loop` remains only
    // for execution outside a runtime quantum, where no handler can suspend. See Task 04/06
    // (`docs/plans/archive/2026-07-13-unified-cooperative-runtime.md`).
    register_fn_ctx(env, "__runtime-quantum?", |_ctx, _args| {
        Ok(Value::bool(sema_core::in_runtime_quantum()))
    });
    register_fn_ctx(env, "__agent-begin", |_ctx, args| agent_begin(args));
    register_runtime_fn_ctx(env, "__agent-step", |ctx, args| {
        let token = agent_token_arg(args, "__agent-step")?;
        agent_step(ctx, token)
    });
    register_runtime_fn_ctx(env, "__agent-exec-tools", |ctx, args| {
        let token = agent_token_arg(args, "__agent-exec-tools")?;
        agent_exec_tools(ctx, token)
    });
    register_fn_ctx(env, "__agent-finish", |_ctx, args| {
        // `(__agent-finish token)` on the normal-done path; `(__agent-finish token
        // err)` from the driver's `catch` when the loop unwound on an error —
        // notably a cancellation, whose bytecode NOW runs the catch (the unified
        // runtime resumes a cancelled parked task to unwind cleanly), so the
        // `invoke_agent` span must be closed carrying the failure status rather
        // than a bare (unset) end.
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("__agent-finish", "1-2", args.len()));
        }
        let token = agent_token_arg(&args[..1], "__agent-finish")?;
        let finish_error = args.get(1).map(|e| e.to_string());
        agent_finish(token, finish_error)
    });

    // `llm/chat`'s `:tools` twin of `__agent-begin`: builds an ordinary agent-loop
    // handle directly from raw messages + an options map (llm/chat has no
    // session/memory/agent-object surface to unpack), so the SAME `__agent-step` /
    // `__agent-exec-tools` / `__agent-finish` / `__agent-drive` machinery above
    // drives it. Returns nil (not a token) when no tool loop is needed, so the
    // prelude dispatcher falls through to `__llm-chat-blocking`, which already
    // offloads the plain-completion case (WP-LLM-SIMPLE). Gated as "llm/chat" —
    // see `register_fn_ctx_gated_as`.
    register_fn_ctx_gated_as(
        env,
        sandbox,
        sema_core::Caps::LLM,
        "__chat-begin",
        "llm/chat",
        |_ctx, args| chat_begin(args),
    );

    register_fn(env, "__agent-stream-apply", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("__agent-stream-apply", "2", args.len()));
        }
        let agent_token = agent_token_arg(&args[..1], "__agent-stream-apply")?;
        agent_stream_apply(agent_token, stream_token_arg(&args[1])?)
    });

    register_fn(env, "tool?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_tool_def_rc().is_some()))
    });

    register_fn(env, "agent?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_agent_rc().is_some()))
    });

    // Tool accessor functions
    register_fn(env, "tool/name", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool/name", "1", args.len()));
        }
        let t = args[0]
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", args[0].type_name()))?;
        Ok(Value::string(&t.name))
    });

    register_fn(env, "tool/description", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool/description", "1", args.len()));
        }
        let t = args[0]
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", args[0].type_name()))?;
        Ok(Value::string(&t.description))
    });

    register_fn(env, "tool/parameters", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool/parameters", "1", args.len()));
        }
        let t = args[0]
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", args[0].type_name()))?;
        Ok(t.parameters.clone())
    });

    register_fn(env, "tool/policy-subjects", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool/policy-subjects", "1", args.len()));
        }
        let tool = args[0]
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", args[0].type_name()))?;
        Ok(Value::vector(
            tool.policy_subjects
                .iter()
                .map(tool_policy_subject_to_value)
                .collect(),
        ))
    });

    // (agent {:system "…" :tools […] :model "…" :max-turns N}) — build an anonymous,
    // reusable actor value (system prompt + tools + model + max-turns) without binding
    // it. The named form is `defagent`; this is the plain constructor used inline (e.g.
    // `(define bot (agent {:tools [t]}))` or passed to a `step` via `:agent`). All opts
    // are optional; the name is empty for an anonymous agent (a `:agent` step falls back
    // to the role label "step" when the name is empty). Mirrors `register_agent`'s opts
    // extraction in sema-eval, the path `defagent` uses.
    register_fn(env, "agent", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent", "1", args.len()));
        }
        let opts = args.map_at(0, "agent")?;
        let system = opts
            .get(&Value::keyword("system"))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let tools = opts
            .get(&Value::keyword("tools"))
            .map(|v| {
                if let Some(l) = v.as_list() {
                    l.to_vec()
                } else if let Some(v) = v.as_vector() {
                    v.to_vec()
                } else {
                    vec![]
                }
            })
            .unwrap_or_default();
        let max_turns = opts
            .get(&Value::keyword("max-turns"))
            .and_then(|v| v.as_int())
            .unwrap_or(10) as usize;
        let model = opts
            .get(&Value::keyword("model"))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let name = opts
            .get(&Value::keyword("name"))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        Ok(Value::agent(Agent {
            name,
            system,
            tools,
            max_turns,
            model,
        }))
    });

    // Agent accessor functions
    register_fn(env, "agent/name", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/name", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::string(&a.name))
    });

    register_fn(env, "agent/system", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/system", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::string(&a.system))
    });

    register_fn(env, "agent/tools", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/tools", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::list(a.tools.clone()))
    });

    register_fn(env, "agent/model", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/model", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::string(&a.model))
    });

    register_fn(env, "agent/max-turns", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/max-turns", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::int(a.max_turns as i64))
    });

    // (tool/invoke tool args) — direct invocation of a deftool handler, without
    // routing through an LLM agent. Custom `:validate` predicates and the handler
    // itself dispatch as structural `NativeOutcome::Call`s so a handler that
    // suspends parks on the runtime like any cooperative work.
    register_runtime_fn_ctx(env, "tool/invoke", |_ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("tool/invoke", "2", args.len()));
        }
        let tool_def = args[0]
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", args[0].type_name()))?;
        if args[1].as_map_rc().is_none() {
            return Err(SemaError::type_error("map", args[1].type_name()));
        }

        // JSON-coerce the arguments (lossily) so a direct invocation hands the
        // handler exactly what an agent-driven tool call would.
        let json_args = sema_core::value_to_json_lossy(&args[1]);
        enforce_direct_tool_policy(&tool_def.name, &json_args, &tool_def.policy_subjects)?;
        let handler_args = json_args_to_sema(&tool_def.parameters, &json_args, &tool_def.handler);
        Box::new(ToolInvokeContinuation {
            tool_name: tool_def.name.clone(),
            steps: prepare_extraction_validation(&args[1], &tool_def.parameters),
            errors: Vec::new(),
            pending_predicate: None,
            handler: Some((tool_def.handler.clone(), handler_args)),
        })
        .advance()
    });
}
