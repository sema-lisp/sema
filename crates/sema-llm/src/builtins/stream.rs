use super::*;

pub(super) enum StreamCassettePlan {
    Replay(ChatResponse),
    Live { record_key: Option<String> },
}

pub(super) fn prepare_stream_cassette(
    request: &ChatRequest,
    chunk_cb: &mut dyn FnMut(&str) -> Result<(), crate::types::LlmError>,
    span: &sema_otel::LlmSpan,
) -> Result<StreamCassettePlan, SemaError> {
    if current_cassette_scope().is_none() {
        return Ok(StreamCassettePlan::Live { record_key: None });
    }

    let key = compute_cache_key(request);
    match cassette_decide(&key).expect("cassette scope checked above") {
        crate::cassette::Decision::Replay(entry) => {
            enforce_stored_model_policy(&entry.provider, &entry.model, PolicySource::Cassette)?;
            let provider = entry.provider.clone();
            let mut response = entry.to_response();
            if output_policy_active() {
                apply_output_policy_to_response(&mut response, PolicySource::Cassette)?;
                if !response.content.is_empty() {
                    chunk_cb(&response.content)
                        .map_err(|error| SemaError::Llm(error.to_string()))?;
                }
            } else {
                for chunk in &entry.chunks {
                    chunk_cb(chunk).map_err(|error| SemaError::Llm(error.to_string()))?;
                }
            }
            span.set_dispatch("cassette", &response.model);
            span.set_response(&response_facts("cassette", &response));
            set_serving_provider(&provider);
            Ok(StreamCassettePlan::Replay(response))
        }
        crate::cassette::Decision::Miss(key) => Err(cassette_miss_error(&key)),
        crate::cassette::Decision::Record => Ok(StreamCassettePlan::Live {
            record_key: Some(key),
        }),
    }
}

pub(super) fn stream_live(
    p: &dyn LlmProvider,
    request: ChatRequest,
    record_key: Option<&str>,
    chunk_cb: &mut dyn FnMut(&str) -> Result<(), crate::types::LlmError>,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    let stream_real = |req: ChatRequest,
                       cb: &mut dyn FnMut(&str) -> Result<(), crate::types::LlmError>|
     -> Result<ChatResponse, SemaError> {
        // Stamp the streaming time-to-first-token on the first chunk delivered by the
        // real provider (mark_first_token is itself idempotent).
        let mut seen_first = false;
        let mut timed = |chunk: &str| -> Result<(), crate::types::LlmError> {
            if !seen_first {
                span.mark_first_token();
                seen_first = true;
            }
            cb(chunk)
        };
        p.stream_complete(req, &mut timed).map_err(|e| {
            span.record_error(llm_error_kind(&e), &e.to_string());
            SemaError::Llm(e.to_string())
        })
    };

    let defer_output = output_policy_active();
    let mut response = if defer_output {
        let mut discarded = |_chunk: &str| -> Result<(), crate::types::LlmError> { Ok(()) };
        stream_real(request.clone(), &mut discarded)?
    } else if let Some(key) = record_key {
        let mut chunks = Vec::new();
        let mut collect = |chunk: &str| -> Result<(), crate::types::LlmError> {
            chunks.push(chunk.to_string());
            chunk_cb(chunk)
        };
        let response = stream_real(request.clone(), &mut collect)?;
        cassette_record(crate::cassette::TapeEntry::from_stream(
            key,
            p.name(),
            &chunks,
            &response,
        ));
        response
    } else {
        stream_real(request.clone(), chunk_cb)?
    };
    if defer_output {
        if let Err(error) = apply_output_policy_to_response(&mut response, PolicySource::Request) {
            track_usage(&response.usage)?;
            return Err(error);
        }
        if let Some(key) = record_key {
            let chunks = if response.content.is_empty() {
                Vec::new()
            } else {
                vec![response.content.clone()]
            };
            cassette_record(crate::cassette::TapeEntry::from_stream(
                key,
                p.name(),
                &chunks,
                &response,
            ));
        }
        if !response.content.is_empty() {
            chunk_cb(&response.content).map_err(|error| SemaError::Llm(error.to_string()))?;
        }
    }
    span.set_dispatch(p.name(), &request.model);
    span.set_response(&response_facts(p.name(), &response));
    Ok(response)
}

/// Parsed `llm/stream`-shaped args: the request, the optional callback, and the
/// optional opts map.
pub(super) type StreamArgs = (
    ChatRequest,
    Option<Value>,
    Option<Rc<BTreeMap<Value, Value>>>,
);

/// Parse `llm/stream`-shaped args — prompt/messages, then an optional callback
/// (any procedure) and an optional opts map in either order — into the
/// `ChatRequest` plus the raw callback/opts. Shared by the blocking native
/// (`__llm-stream-blocking`) and the non-blocking `__stream-begin`.
pub(super) fn parse_stream_args(args: &[Value]) -> Result<StreamArgs, SemaError> {
    if args.is_empty() || args.len() > 3 {
        return Err(SemaError::arity("llm/stream", "1-3", args.len()));
    }

    let messages = if let Some(s) = args[0].as_str() {
        vec![ChatMessage::new("user", s)]
    } else if let Some(p) = args[0].as_prompt_rc() {
        p.messages
            .iter()
            .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
            .collect()
    } else if args[0].as_seq().is_some() {
        extract_messages(&args[0])?
    } else {
        return Err(SemaError::type_error(
            "string, prompt, or messages",
            args[0].type_name(),
        ));
    };

    let mut callback: Option<Value> = None;
    let mut opts_map: Option<Rc<BTreeMap<Value, Value>>> = None;
    for arg in &args[1..] {
        if arg.as_lambda_rc().is_some() || arg.as_native_fn_rc().is_some() {
            callback = Some(arg.clone());
        } else if let Some(m) = arg.as_map_rc() {
            opts_map = Some(m);
        }
    }

    let mut model = String::new();
    let mut max_tokens = None;
    let mut temperature = None;
    let mut system = None;
    if let Some(ref opts) = opts_map {
        model = opts.opt_str("model").unwrap_or_default();
        max_tokens = opts.opt_int("max-tokens").map(|n| n as u32);
        temperature = opts.opt_f64("temperature");
        system = opts.opt_str("system");
    }

    let mut request = ChatRequest::new(model, messages);
    request.max_tokens = max_tokens.or(Some(4096));
    request.temperature = temperature;
    request.system = system;
    Ok((request, callback, opts_map))
}

/// Stream-open budget pre-gate. When `:on-stream :pre-gate` is active, refuse to OPEN a
/// stream if the scope's spend is already at/over the cost or token limit. (A stream's own
/// cost is unknown until it ends, so this is the only honest gate — a single in-flight
/// stream can still push past the cap, but the next call is blocked.)
pub(super) fn stream_budget_pregate() -> Result<(), SemaError> {
    if !STREAM_BUDGET_PREGATE.with(|c| c.get()) {
        return Ok(());
    }
    let Some(frame) = active_budget() else {
        return Ok(());
    };
    let f = frame.borrow();
    if let Some(limit) = f.cost_limit {
        let spent = f.cost_spent;
        if spent >= limit {
            return Err(SemaError::Llm(format!(
                "budget exceeded: ${spent:.4} of ${limit:.4} limit already spent — \
                 streaming call blocked at open"
            )));
        }
    }
    if let Some(limit) = f.token_limit {
        let spent = f.tokens_spent;
        if spent >= limit {
            return Err(SemaError::Llm(format!(
                "token budget exceeded: {spent} of {limit} tokens already used — \
                 streaming call blocked at open"
            )));
        }
    }
    Ok(())
}

/// Open a stream against one fallback-chain provider (resolving its per-entry model).
pub(super) fn stream_one_provider(
    entry: &FallbackEntry,
    mut request: ChatRequest,
    chunk_cb: &mut dyn FnMut(&str) -> Result<(), crate::types::LlmError>,
    span: &sema_otel::LlmSpan,
) -> Result<Option<ChatResponse>, SemaError> {
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = reg.get(&entry.provider).ok_or_else(|| {
            SemaError::Llm(format!("fallback provider '{}' not found", entry.provider))
        })?;
        if let Some(model) = &entry.model {
            request.model = model.clone();
        } else if request.model.is_empty() {
            request.model = provider.default_model().to_string();
        }
        let record_key = match prepare_stream_cassette(&request, chunk_cb, span)? {
            StreamCassettePlan::Replay(response) => return Ok(Some(response)),
            StreamCassettePlan::Live { record_key } => record_key,
        };
        if !model_target_allowed(&entry.provider, &request.model, PolicySource::Request, true)? {
            return Ok(None);
        }
        let resp = stream_live(&*provider, request, record_key.as_deref(), chunk_cb, span)?;
        set_serving_provider(&entry.provider);
        Ok(Some(resp))
    })
}

/// Stream-open dispatch for `llm/stream`: budget pre-gate + rate-limit, then open the
/// stream through the fallback chain. Fails over to the next provider ONLY if a provider
/// errors *before emitting any chunk*; once a chunk is delivered a mid-stream error
/// surfaces (failing over would re-emit the already-delivered partial — see the spike test
/// `spike_mid_stream_failure_behaviour`).
pub(super) fn stream_with_dispatch(
    mut request: ChatRequest,
    chunk_cb: &mut dyn FnMut(&str) -> Result<(), crate::types::LlmError>,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    apply_input_policy_to_request(&mut request)?;
    stream_budget_pregate()?;
    enforce_rate_limit();

    let chain = FALLBACK_CHAIN.with(|c| c.borrow().clone());
    match chain {
        Some(chain) if !chain.is_empty() => {
            let mut last_error = None;
            for entry in &chain {
                let mut emitted = false;
                let result = {
                    let mut wrapped = |c: &str| -> Result<(), crate::types::LlmError> {
                        emitted = true;
                        chunk_cb(c)
                    };
                    stream_one_provider(entry, request.clone(), &mut wrapped, span)
                };
                match result {
                    Ok(Some(resp)) => return Ok(resp),
                    Ok(None) => continue,
                    Err(e) if emitted => {
                        // Mid-stream failure: surface; do NOT fail over (would duplicate).
                        span.record_error("provider_error", &e.to_string());
                        return Err(e);
                    }
                    Err(e) => {
                        eprintln!(
                            "Provider '{}' failed to open stream: {e}, trying next...",
                            entry.provider
                        );
                        last_error = Some(e);
                    }
                }
            }
            let err = last_error.unwrap_or_else(|| SemaError::Llm("all providers failed".into()));
            span.record_error("provider_error", &err.to_string());
            Err(err)
        }
        _ => with_provider(|p| {
            let mut req = request;
            if req.model.is_empty() {
                req.model = p.default_model().to_string();
            }
            let record_key = match prepare_stream_cassette(&req, chunk_cb, span)? {
                StreamCassettePlan::Replay(response) => return Ok(response),
                StreamCassettePlan::Live { record_key } => record_key,
            };
            model_target_allowed(p.name(), &req.model, PolicySource::Request, false)?;
            stream_live(p, req, record_key.as_deref(), chunk_cb, span)
        }),
    }
}

/// One event on a stream run's wire channel.
pub(super) enum StreamEvent {
    /// A text delta, in arrival order.
    Delta(String),
    /// Terminal event: the assembled response (or the stream failure) plus the
    /// registry name of the provider that served (or last attempted) it.
    Done(Box<StreamDone>),
}

pub(super) struct StreamDone {
    result: Result<ChatResponse, LlmError>,
    provider: String,
}

/// Provider-at-a-time dispatch state retained between `__stream-next` calls.
/// It contains host data only; Sema callbacks are looked up immediately before
/// a structural `NativeOutcome::Call` and are traced by that call itself.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct StreamDispatchState {
    chain: Vec<ResolvedProvider>,
    /// Model `:skip` applies to every explicit fallback chain, including one entry.
    explicit_fallback: bool,
    request: ChatRequest,
    next_provider: usize,
    last_error: Option<(LlmError, String)>,
    rate_limit_wait_ms: u64,
}

/// Per-run state for a non-blocking stream, keyed by an integer token in the
/// thread-local `STREAM_RUNS` slab (its own slab — agent-loop state is not
/// touched). Owns the wire receiver and everything the finalize needs on the VM
/// thread after the last park.
pub(super) struct StreamRunState {
    /// Wire-side receiver (`None` for pre-filled runs: cassette replay).
    pub(super) rx: Option<std::sync::mpsc::Receiver<StreamEvent>>,
    /// Pre-filled events, drained before `rx`.
    pub(super) buffered: std::collections::VecDeque<StreamEvent>,
    /// Real provider dispatch. `Some` + no receiver means the next provider is
    /// ready to start; `Some` + a receiver means one native stream is active.
    /// `None` means cassette replay or a terminal event is already buffered.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) dispatch: Option<StreamDispatchState>,
    /// Detached chat span (parent captured at begin), finalized when `Done` lands.
    pub(super) span: Option<sema_otel::LlmSpan>,
    /// This leaf's usage-accumulator frame, captured at begin so accounting uses
    /// the dispatch-time scope.
    pub(super) usage_accum_slot: Option<Rc<RefCell<LeafUsage>>>,
    /// The dispatch-time budget frame `Rc` (ASYNC-1), re-installed around the
    /// finalize's `track_usage`.
    pub(super) budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
    /// Set when a cassette is recording; the entry is written at finalize with
    /// the collected chunk boundaries.
    pub(super) cassette_record_key: Option<String>,
    /// The exact cassette selected at dispatch, retained because finalization can
    /// run after the task's dynamic scope has been swapped out.
    pub(super) cassette_scope: Option<CassetteScope>,
    /// Every delta drained so far (cassette recording preserves boundaries).
    pub(super) collected: Vec<String>,
    /// Output policies require terminal buffering so an unsafe prefix can never
    /// escape before the assembled response is checked.
    pub(super) defer_deltas: bool,
    pub(super) first_token_seen: bool,
    /// The assembled response, set once `Done(Ok)` has been finalized.
    pub(super) response: Option<ChatResponse>,
    pub(super) done: bool,
    /// The scheduler task that opened this run (None outside a task). The
    /// task-reaped sweep reclaims entries by this id when their task is
    /// cancelled — `__stream-finish` cannot run for a cancelled task.
    pub(super) owning_task_id: Option<RuntimeTaskId>,
    /// A failure that arrived in a batch that still carried deltas: stored so the
    /// driver delivers those deltas to the callback first, then raised (and the
    /// entry dropped) on the next `__stream-next`/`__stream-finish`.
    pub(super) pending_error: Option<SemaError>,
}

impl Drop for StreamRunState {
    fn drop(&mut self) {
        // Normal path (finalize already took the span, or `reset_runtime_state`
        // during eval): let the detached span end with the otel thread-locals
        // alive. Thread teardown of a leaked (cancelled) run: forget the span
        // rather than let its `Drop` touch dead TLS and abort the process
        // (mirrors `AgentLoopState`).
        if !sema_otel::tls_alive() {
            std::mem::forget(self.span.take());
        }
    }
}

thread_local! {
    /// Live non-blocking stream runs, keyed by the integer token handed to Sema.
    pub(super) static STREAM_RUNS: RefCell<std::collections::HashMap<u64, StreamRunState>> =
        RefCell::new(std::collections::HashMap::new());
    pub(super) static STREAM_RUN_NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

/// Clear any live stream-run state (called from `reset_runtime_state`). A task
/// cancelled while parked in `__stream-next` abandons its entry here (the wire
/// worker keeps streaming into the never-drained channel and is discarded —
/// best-effort, like completion offloads), so this is also the leak backstop.
pub(super) fn clear_stream_runs() {
    STREAM_RUNS.with(|r| r.borrow_mut().clear());
    STREAM_RUN_NEXT_ID.with(|c| c.set(1));
}

/// Extract the integer token from a `__stream-*` native's arg.
pub(super) fn stream_token_arg(v: &Value) -> Result<u64, SemaError> {
    v.as_int()
        .filter(|n| *n >= 0)
        .map(|n| n as u64)
        .ok_or_else(|| SemaError::type_error("stream-run-handle", v.type_name()))
}

/// Walk the resolved provider chain, opening the stream against each provider in
/// turn and emitting `StreamEvent`s. Fail-over happens ONLY before the first
/// delta; once a chunk is out a mid-stream error surfaces (failing over would
/// re-emit the already-delivered partial — same policy as
/// `stream_with_dispatch`). Runs on a pool worker, so it touches NO
/// thread-locals; always ends with exactly one `Done`.
pub(super) fn stream_wire_walk(
    chain: &[ResolvedProvider],
    request: &ChatRequest,
    emit: &mut dyn FnMut(StreamEvent),
) {
    let mut last: Option<(LlmError, String)> = None;
    for entry in chain {
        let mut req = request.clone();
        if let Some(m) = &entry.model {
            req.model = m.clone();
        } else if req.model.is_empty() {
            req.model = entry.provider.default_model().to_string();
        }
        let mut emitted = false;
        let result = {
            let mut cb = |c: &str| -> Result<(), LlmError> {
                emitted = true;
                emit(StreamEvent::Delta(c.to_string()));
                Ok(())
            };
            entry.provider.stream_complete(req, &mut cb)
        };
        match result {
            Ok(resp) => {
                emit(StreamEvent::Done(Box::new(StreamDone {
                    result: Ok(resp),
                    provider: entry.name.clone(),
                })));
                return;
            }
            Err(e) if emitted => {
                // Mid-stream failure: surface; do NOT fail over (would duplicate).
                emit(StreamEvent::Done(Box::new(StreamDone {
                    result: Err(e),
                    provider: entry.name.clone(),
                })));
                return;
            }
            Err(e) => {
                last = Some((e, entry.name.clone()));
            }
        }
    }
    let (e, name) = last.unwrap_or_else(|| {
        (
            LlmError::Config("all providers failed".to_string()),
            String::new(),
        )
    });
    emit(StreamEvent::Done(Box::new(StreamDone {
        result: Err(e),
        provider: name,
    })));
}

/// Drive one native provider attempt on the I/O pool. Fallback selection stays
/// on the VM thread, so this helper reports exactly one terminal event and does
/// not inspect or log the remainder of the chain.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn stream_wire_attempt(
    entry: &ResolvedProvider,
    request: &ChatRequest,
    emit: &mut dyn FnMut(StreamEvent),
) {
    stream_wire_walk(std::slice::from_ref(entry), request, emit);
}

/// The conversation after one exchange: `conv` plus the user turn and the
/// assistant reply, with the reply's usage folded into the metadata. Runs on
/// the VM thread after the response lands, never inside an offload.
pub(super) fn conversation_with_exchange(
    conv: &Conversation,
    user_msg: String,
    response: ChatResponse,
) -> Value {
    let mut messages = conv.messages.clone();
    messages.push(Message {
        role: Role::User,
        content: user_msg,
        images: Vec::new(),
    });
    messages.push(Message {
        role: Role::Assistant,
        content: response.content,
        images: Vec::new(),
    });
    let mut metadata = conv.metadata.clone();
    accumulate_usage(&mut metadata, &response.usage);
    Value::conversation(Conversation {
        messages,
        model: conv.model.clone(),
        metadata,
    })
}

/// Resolve the active fallback chain (or the default provider) into owned `Arc`
/// clones on the VM thread. The boolean records whether the chain was explicit,
/// so the offloaded wire walk touches no thread-locals.
pub(super) fn resolve_stream_chain() -> Result<(Vec<ResolvedProvider>, bool), SemaError> {
    let fallback = FALLBACK_CHAIN.with(|c| c.borrow().clone());
    let explicit_fallback = fallback.as_ref().is_some_and(|entries| !entries.is_empty());
    let chain = PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        match fallback {
            Some(entries) if !entries.is_empty() => entries
                .iter()
                .map(|e| {
                    reg.get(&e.provider)
                        .map(|p| ResolvedProvider {
                            provider: p,
                            name: e.provider.clone(),
                            model: e.model.clone(),
                        })
                        .ok_or_else(|| {
                            SemaError::Llm(format!("fallback provider '{}' not found", e.provider))
                        })
                })
                .collect::<Result<Vec<_>, _>>(),
            _ => {
                let p = reg.default_provider().ok_or_else(|| {
                    SemaError::Llm(
                        "no LLM provider configured. Use (llm/configure :anthropic \
                         {:api-key ...}) first"
                            .to_string(),
                    )
                })?;
                let name = p.name().to_string();
                Ok(vec![ResolvedProvider {
                    provider: p,
                    name,
                    model: None,
                }])
            }
        }
    })?;
    Ok((chain, explicit_fallback))
}

/// Start a non-blocking stream run. Cassette replay does not reserve a rate-limit slot.
pub(super) fn stream_run_begin(
    mut request: ChatRequest,
    span: sema_otel::LlmSpan,
) -> Result<Value, SemaError> {
    apply_input_policy_to_request(&mut request)?;
    stream_budget_pregate()?;
    let defer_deltas = output_policy_active();

    let cassette_scope = current_cassette_scope();
    let cassette_decision = cassette_scope.as_ref().map(|scope| {
        let key = compute_cache_key(&request);
        (key.clone(), scope.borrow().decide(&key))
    });
    let mut buffered = std::collections::VecDeque::new();
    let mut cassette_record_key = None;
    let mut prefilled = false;
    match cassette_decision {
        Some((_, crate::cassette::Decision::Replay(entry))) => {
            enforce_stored_model_policy(&entry.provider, &entry.model, PolicySource::Cassette)?;
            for ch in &entry.chunks {
                buffered.push_back(StreamEvent::Delta(ch.clone()));
            }
            buffered.push_back(StreamEvent::Done(Box::new(StreamDone {
                result: Ok(entry.to_response()),
                provider: "cassette".to_string(),
            })));
            prefilled = true;
        }
        Some((k, crate::cassette::Decision::Miss(_))) => return Err(cassette_miss_error(&k)),
        Some((k, crate::cassette::Decision::Record)) => cassette_record_key = Some(k),
        _ => {}
    }

    #[cfg(not(target_arch = "wasm32"))]
    let dispatch = if prefilled {
        None
    } else {
        let (chain, explicit_fallback) = resolve_stream_chain()?;
        Some(StreamDispatchState {
            chain,
            explicit_fallback,
            request,
            next_provider: 0,
            last_error: None,
            rate_limit_wait_ms: reserve_rate_limit_wait_ms(),
        })
    };

    #[cfg(not(target_arch = "wasm32"))]
    let rx = None;

    #[cfg(target_arch = "wasm32")]
    let rx = if prefilled {
        None
    } else {
        let rate_limit_wait_ms = reserve_rate_limit_wait_ms();
        let (chain, _) = resolve_stream_chain()?;
        let (tx, rx) = std::sync::mpsc::channel::<StreamEvent>();
        if rate_limit_wait_ms > 0 {
            sema_core::blocking_sleep_ms(rate_limit_wait_ms);
        }
        let mut emit = |ev: StreamEvent| {
            let _ = tx.send(ev);
        };
        stream_wire_walk(&chain, &request, &mut emit);
        Some(rx)
    };

    let state = StreamRunState {
        rx,
        buffered,
        #[cfg(not(target_arch = "wasm32"))]
        dispatch,
        span: Some(span),
        usage_accum_slot: current_usage_accum(),
        budget_slot: active_budget(),
        cassette_record_key,
        cassette_scope,
        collected: Vec::new(),
        defer_deltas,
        first_token_seen: false,
        response: None,
        done: false,
        owning_task_id: sema_core::current_task_id(),
        pending_error: None,
    };
    let token = STREAM_RUN_NEXT_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    });
    STREAM_RUNS.with(|r| r.borrow_mut().insert(token, state));
    Ok(Value::int(token as i64))
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) enum RuntimeStreamPhase {
    Ready,
    Pacing,
    Sema { provider: String, model: String },
}

/// Token-only continuation that advances one stream provider at a time. The run
/// slab owns all host state; `NativeCall` owns and traces each Sema callback and
/// request value while that call is active.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct RuntimeStreamDriver {
    token: u64,
    phase: RuntimeStreamPhase,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for RuntimeStreamDriver {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeStreamDriver {
    fn advance(mut self: Box<Self>) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeCall, NativeOutcome, NativeSuspend, WaitKind};

        enum Action {
            Pace(u64),
            Skip,
            Sema {
                provider: String,
                model: String,
                request: ChatRequest,
            },
            Native {
                entry: ResolvedProvider,
                request: ChatRequest,
            },
            Terminal,
        }

        loop {
            let action = STREAM_RUNS.with(|runs| -> Result<Action, SemaError> {
                let mut runs = runs.borrow_mut();
                let state = runs
                    .get_mut(&self.token)
                    .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
                let dispatch = state.dispatch.as_mut().ok_or_else(|| {
                    SemaError::eval("stream dispatch resumed after reaching a terminal state")
                })?;

                if dispatch.rate_limit_wait_ms > 0 {
                    let wait_ms = std::mem::take(&mut dispatch.rate_limit_wait_ms);
                    return Ok(Action::Pace(wait_ms));
                }

                let Some(entry) = dispatch.chain.get(dispatch.next_provider).cloned() else {
                    let (error, provider) = dispatch.last_error.take().unwrap_or_else(|| {
                        (
                            LlmError::Config("all providers failed".to_string()),
                            String::new(),
                        )
                    });
                    state.dispatch = None;
                    state
                        .buffered
                        .push_back(StreamEvent::Done(Box::new(StreamDone {
                            result: Err(error),
                            provider,
                        })));
                    return Ok(Action::Terminal);
                };
                dispatch.next_provider += 1;

                let mut request = dispatch.request.clone();
                if let Some(model) = &entry.model {
                    request.model = model.clone();
                } else if request.model.is_empty() {
                    request.model = entry.provider.default_model().to_string();
                }
                if !model_target_allowed(
                    &entry.name,
                    &request.model,
                    PolicySource::Request,
                    dispatch.explicit_fallback,
                )? {
                    return Ok(Action::Skip);
                }

                if entry.provider.runs_on_vm_thread() {
                    Ok(Action::Sema {
                        provider: entry.name,
                        model: request.model.clone(),
                        request,
                    })
                } else {
                    Ok(Action::Native { entry, request })
                }
            })?;

            match action {
                Action::Skip => continue,
                Action::Pace(wait_ms) => {
                    self.phase = RuntimeStreamPhase::Pacing;
                    return Ok(NativeOutcome::Suspend(NativeSuspend {
                        wait: WaitKind::Timer(std::time::Duration::from_millis(wait_ms)),
                        continuation: self,
                    }));
                }
                Action::Sema {
                    provider,
                    model,
                    request,
                } => {
                    let callback = match lisp_provider_complete_callback(&provider) {
                        Ok(callback) => callback,
                        Err(error) => {
                            stream_note_open_failure(self.token, provider, error)?;
                            continue;
                        }
                    };
                    self.phase = RuntimeStreamPhase::Sema { provider, model };
                    return Ok(NativeOutcome::Call(NativeCall {
                        callable: callback,
                        args: vec![chat_request_to_value(&request)],
                        continuation: self,
                    }));
                }
                Action::Native { entry, request } => {
                    let (tx, rx) = std::sync::mpsc::channel::<StreamEvent>();
                    STREAM_RUNS.with(|runs| -> Result<(), SemaError> {
                        let mut runs = runs.borrow_mut();
                        let state = runs.get_mut(&self.token).ok_or_else(|| {
                            SemaError::Llm("stream-run handle not found".to_string())
                        })?;
                        state.rx = Some(rx);
                        Ok(())
                    })?;
                    sema_io::io_spawn_blocking(move || {
                        let mut emit = |event| {
                            let _ = tx.send(event);
                        };
                        stream_wire_attempt(&entry, &request, &mut emit);
                    });
                    return stream_next_runtime_step(self.token);
                }
                Action::Terminal => return stream_next_runtime_step(self.token),
            }
        }
    }

    fn sema_succeeded(
        &self,
        provider: String,
        response: ChatResponse,
    ) -> sema_core::runtime::NativeResult {
        STREAM_RUNS.with(|runs| -> Result<(), SemaError> {
            let mut runs = runs.borrow_mut();
            let state = runs
                .get_mut(&self.token)
                .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
            state.dispatch = None;
            state
                .buffered
                .push_back(StreamEvent::Delta(response.content.clone()));
            state
                .buffered
                .push_back(StreamEvent::Done(Box::new(StreamDone {
                    result: Ok(response),
                    provider,
                })));
            Ok(())
        })?;
        stream_next_runtime_step(self.token)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::NativeContinuation for RuntimeStreamDriver {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;

        match (&self.phase, input) {
            (RuntimeStreamPhase::Pacing, ResumeInput::Returned(_)) => {
                let mut this = self;
                this.phase = RuntimeStreamPhase::Ready;
                this.advance()
            }
            (RuntimeStreamPhase::Sema { provider, model }, ResumeInput::Returned(value)) => {
                let provider = provider.clone();
                let model = model.clone();
                match parse_lisp_provider_response(&value, &model) {
                    Ok(response) => self.sema_succeeded(provider, response),
                    Err(error) => {
                        stream_note_open_failure(self.token, provider, error)?;
                        self.advance()
                    }
                }
            }
            (RuntimeStreamPhase::Sema { provider, .. }, ResumeInput::Failed(error)) => {
                let provider = provider.clone();
                stream_note_open_failure(
                    self.token,
                    provider,
                    LlmError::Api {
                        status: 0,
                        message: error.to_string(),
                    },
                )?;
                self.advance()
            }
            (_, ResumeInput::Failed(error)) => Err(error),
            (_, ResumeInput::Cancelled(reason)) => Err(SemaError::eval(format!(
                "stream provider dispatch was cancelled ({reason:?})"
            ))),
            (_, ResumeInput::Runtime(_)) => Err(SemaError::eval(
                "stream provider driver received an unexpected runtime response",
            )),
            (RuntimeStreamPhase::Ready, ResumeInput::Returned(_)) => Err(SemaError::eval(
                "stream provider driver resumed without an active attempt",
            )),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn stream_note_open_failure(
    token: u64,
    provider: String,
    error: LlmError,
) -> Result<(), SemaError> {
    eprintln!("Provider '{provider}' failed to open stream: {error}, trying next...");
    STREAM_RUNS.with(|runs| {
        let mut runs = runs.borrow_mut();
        let state = runs
            .get_mut(&token)
            .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
        let dispatch = state.dispatch.as_mut().ok_or_else(|| {
            SemaError::eval("stream provider failed after reaching a terminal state")
        })?;
        dispatch.last_error = Some((error, provider));
        state.rx = None;
        Ok(())
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn stream_dispatch_ready(token: u64) -> Result<bool, SemaError> {
    STREAM_RUNS.with(|runs| {
        let runs = runs.borrow();
        let state = runs
            .get(&token)
            .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
        Ok(state.dispatch.is_some() && state.rx.is_none())
    })
}

/// Post-stream work on the VM thread when `Done` lands: span finalization,
/// serving-provider stamp, cassette record, per-leaf usage fold, and
/// budget-installed `track_usage` (exactly once per streamed completion).
/// Returns the response, or the error message to surface.
pub(super) struct StreamFinalizeContext {
    span: Option<sema_otel::LlmSpan>,
    usage_accum_slot: Option<Rc<RefCell<LeafUsage>>>,
    budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
    cassette_record_key: Option<String>,
    cassette_scope: Option<CassetteScope>,
    collected: Vec<String>,
    defer_deltas: bool,
}

pub(super) fn stream_finalize(
    done: StreamDone,
    context: StreamFinalizeContext,
) -> Result<ChatResponse, SemaError> {
    let StreamFinalizeContext {
        span,
        usage_accum_slot,
        budget_slot,
        cassette_record_key,
        cassette_scope,
        collected,
        defer_deltas,
    } = context;
    match done.result {
        Ok(mut resp) => {
            if defer_deltas {
                let source = if done.provider == "cassette" {
                    PolicySource::Cassette
                } else {
                    PolicySource::Request
                };
                if let Err(error) = apply_output_policy_to_response(&mut resp, source) {
                    if let Some(span) = span {
                        span.set_dispatch(&done.provider, &resp.model);
                        span.record_error("policy", &error.to_string());
                    }
                    if let Some(slot) = &usage_accum_slot {
                        let cost = pricing::calculate_cost_for(&done.provider, &resp.usage);
                        accumulate_into(slot, &resp.usage, cost);
                    }
                    let previous_budget = ACTIVE_BUDGET
                        .with(|active| std::mem::replace(&mut *active.borrow_mut(), budget_slot));
                    let track_result = USAGE_ACCUM_SUPPRESS.with(|suppress| {
                        suppress.set(true);
                        let result = track_usage(&resp.usage);
                        suppress.set(false);
                        result
                    });
                    ACTIVE_BUDGET.with(|active| *active.borrow_mut() = previous_budget);
                    track_result?;
                    return Err(error);
                }
            }
            if let Some(span) = span {
                span.set_dispatch(&done.provider, &resp.model);
                span.set_response(&response_facts(&done.provider, &resp));
                // span drops here → ends the span.
            }
            // A cassette replay made no provider call — leave the serving stamp
            // alone (matches the sync no-chain path's canonical pricing).
            if done.provider != "cassette" && !done.provider.is_empty() {
                set_serving_provider(&done.provider);
            }
            if let Some(key) = &cassette_record_key {
                let guarded_chunks;
                let recorded_chunks = if defer_deltas {
                    guarded_chunks = if resp.content.is_empty() {
                        Vec::new()
                    } else {
                        vec![resp.content.clone()]
                    };
                    guarded_chunks.as_slice()
                } else {
                    &collected
                };
                cassette_scope_record(
                    &cassette_scope,
                    crate::cassette::TapeEntry::from_stream(
                        key,
                        &done.provider,
                        recorded_chunks,
                        &resp,
                    ),
                );
            }
            // Fold into THIS run's captured accumulator frame, then suppress
            // `track_usage`'s own fold — the finalizer runs outside the per-task
            // install boundary, so the thread-local may hold a sibling's scope.
            if let Some(slot) = &usage_accum_slot {
                let cost = pricing::calculate_cost_for(&done.provider, &resp.usage);
                accumulate_into(slot, &resp.usage, cost);
            }
            let track_result = {
                let prev_budget = ACTIVE_BUDGET
                    .with(|b| std::mem::replace(&mut *b.borrow_mut(), budget_slot.clone()));
                let r = USAGE_ACCUM_SUPPRESS.with(|s| {
                    s.set(true);
                    let r = track_usage(&resp.usage);
                    s.set(false);
                    r
                });
                ACTIVE_BUDGET.with(|b| *b.borrow_mut() = prev_budget);
                r
            };
            track_result.map(|()| resp)
        }
        Err(e) => {
            if let Some(span) = span {
                span.record_error(llm_error_kind(&e), &e.to_string());
            }
            Err(SemaError::Llm(e.to_string()))
        }
    }
}

/// Build the `{:deltas [...] :done bool}` batch map `__stream-next` resolves to.
pub(super) fn stream_batch_map(deltas: Vec<Value>, done: bool) -> Value {
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("deltas"), Value::list(deltas));
    map.insert(Value::keyword("done"), Value::bool(done));
    Value::map(map)
}

/// Drain every currently-available wire event for `token` into one batch
/// (batching amortizes park/resume over fast token streams), finalizing the run
/// when `Done` arrives. `blocking` waits for the first event (the sync-context
/// fallback and pre-filled runs); the nonblocking scan never blocks. `Ok(None)` =
/// nothing available yet (stay parked).
///
/// A failure is stored as `pending_error` and raised by the next `__stream-next`
/// as an ordinary native error. This keeps it catchable in task context and lets
/// the callback observe every delta delivered before the failure.
pub(super) fn stream_poll_batch(token: u64, blocking: bool) -> Result<Option<Value>, SemaError> {
    use std::sync::mpsc::TryRecvError;

    let mut batch: Vec<Value> = Vec::new();
    let mut done_event: Option<Box<StreamDone>> = None;
    let mut closed = false;

    // Short-borrow the slab: drain buffered events + the channel into the batch.
    STREAM_RUNS.with(|r| -> Result<(), SemaError> {
        let mut slab = r.borrow_mut();
        let st = slab
            .get_mut(&token)
            .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
        loop {
            let ev = if let Some(ev) = st.buffered.pop_front() {
                Some(ev)
            } else if let Some(rx) = &st.rx {
                if blocking && batch.is_empty() {
                    match rx.recv() {
                        Ok(ev) => Some(ev),
                        Err(_) => {
                            closed = true;
                            None
                        }
                    }
                } else {
                    match rx.try_recv() {
                        Ok(ev) => Some(ev),
                        Err(TryRecvError::Empty) => None,
                        Err(TryRecvError::Disconnected) => {
                            closed = true;
                            None
                        }
                    }
                }
            } else {
                None
            };
            match ev {
                Some(StreamEvent::Delta(s)) => {
                    if !st.first_token_seen {
                        st.first_token_seen = true;
                        if let Some(span) = st.span.as_ref() {
                            span.mark_first_token();
                        }
                    }
                    if !st.defer_deltas {
                        batch.push(Value::string(&s));
                    }
                    st.collected.push(s);
                }
                Some(StreamEvent::Done(d)) => {
                    done_event = Some(d);
                    break;
                }
                None => break,
            }
        }
        Ok(())
    })?;

    // A closed channel without a Done means the worker died mid-stream.
    if done_event.is_none() && closed {
        done_event = Some(Box::new(StreamDone {
            result: Err(LlmError::Config("stream: io worker dropped".to_string())),
            provider: String::new(),
        }));
    }

    let Some(done) = done_event else {
        return if batch.is_empty() {
            Ok(None)
        } else {
            Ok(Some(stream_batch_map(batch, false)))
        };
    };

    #[cfg(not(target_arch = "wasm32"))]
    let should_fallback = STREAM_RUNS.with(|runs| {
        let runs = runs.borrow();
        runs.get(&token)
            .is_some_and(|state| !state.first_token_seen && state.dispatch.is_some())
    });
    #[cfg(not(target_arch = "wasm32"))]
    if should_fallback && done.result.is_err() {
        let StreamDone { result, provider } = *done;
        let error = result.expect_err("stream fallback branch requires an error");
        stream_note_open_failure(token, provider, error)?;
        debug_assert!(batch.is_empty());
        return Ok(None);
    }

    // Done landed: take the finalize context out (short-borrow), run the
    // finalize outside the slab borrow, then write the outcome back.
    let ctx = STREAM_RUNS.with(|r| {
        let mut slab = r.borrow_mut();
        slab.get_mut(&token).map(|st| {
            #[cfg(not(target_arch = "wasm32"))]
            {
                st.dispatch = None;
            }
            st.rx = None;
            (
                st.span.take(),
                st.usage_accum_slot.take(),
                st.budget_slot.take(),
                st.cassette_record_key.take(),
                st.cassette_scope.take(),
                std::mem::take(&mut st.collected),
                st.defer_deltas,
            )
        })
    });
    let Some((span, usage_slot, budget_slot, record_key, cassette_scope, collected, defer_deltas)) =
        ctx
    else {
        return Err(SemaError::Llm("stream-run handle not found".to_string()));
    };
    match stream_finalize(
        *done,
        StreamFinalizeContext {
            span,
            usage_accum_slot: usage_slot,
            budget_slot,
            cassette_record_key: record_key,
            cassette_scope,
            collected,
            defer_deltas,
        },
    ) {
        Ok(resp) => {
            if defer_deltas && !resp.content.is_empty() {
                batch.push(Value::string(&resp.content));
            }
            STREAM_RUNS.with(|r| {
                if let Some(st) = r.borrow_mut().get_mut(&token) {
                    st.response = Some(resp);
                    st.done = true;
                }
            });
            Ok(Some(stream_batch_map(batch, true)))
        }
        Err(error) => {
            STREAM_RUNS.with(|r| {
                if let Some(st) = r.borrow_mut().get_mut(&token) {
                    st.pending_error = Some(error);
                }
            });
            Ok(Some(stream_batch_map(batch, false)))
        }
    }
}

/// `__stream-next(token) → {:deltas [str…] :done bool}`. In a runtime task it
/// suspends on an External wait; the decoder drains all currently available
/// deltas per wake (see `stream_poll_batch`). Pre-filled runs and synchronous
/// contexts drain blockingly.
/// Completion-kind tag for the `__stream-next` cooperative inter-scan sleep.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const STREAM_POLL_COMPLETION_KIND: u64 = 0x7374_726d; // "strm"
/// Milliseconds slept off the VM thread between stream-batch scans.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const STREAM_POLL_INTERVAL_MS: u64 = 5;
/// Bounded cleanup deadline for the inter-scan sleep job.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const STREAM_POLL_CLEANUP_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(120);

/// Nil decoder for the `__stream-next` inter-scan sleep — the batch re-check lives
/// in [`StreamPollContinuation`]; the sleep itself carries no result.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct StreamPollDecoder;

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for StreamPollDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::CompletionDecoder for StreamPollDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> sema_core::runtime::DecodedCompletion {
        match result {
            Ok(_) => Ok(Value::nil()),
            Err(failure) => Err(SemaError::eval(failure.message().to_string())),
        }
    }
}

/// Resumes a parked `__stream-next` after its inter-scan sleep: re-scan the run's
/// delta batch (start the next scan, or resolve). Holds only the run `token`
/// (`u64`), so it emits no GC edges.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct StreamPollContinuation {
    token: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for StreamPollContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::NativeContinuation for StreamPollContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;
        match input {
            ResumeInput::Returned(_) => stream_next_runtime_step(self.token),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "__stream-next was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "__stream-next continuation received an unexpected runtime response",
            )),
        }
    }
}

/// One cooperative scan of a stream run under the unified runtime: re-check the
/// delta batch on the VM thread; resolve if a batch is ready, else re-arm the
/// short inter-scan sleep so sibling tasks overlap while the provider streams.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn stream_next_runtime_step(token: u64) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{
        CompletionKind, NativeOutcome, NativeSuspend, PreparedExternalOperation, QuarantineBound,
        SendPayload, WaitKind,
    };
    match stream_poll_batch(token, false) {
        Ok(Some(v)) => return Ok(NativeOutcome::Return(v)),
        Ok(None) => {}
        Err(e) => return Err(e),
    }
    if stream_dispatch_ready(token)? {
        return Box::new(RuntimeStreamDriver {
            token,
            phase: RuntimeStreamPhase::Ready,
        })
        .advance();
    }
    let kind = CompletionKind::try_from_raw(STREAM_POLL_COMPLETION_KIND)
        .expect("stream poll completion kind is nonzero");
    let bound = QuarantineBound::hard_deadline(STREAM_POLL_CLEANUP_DEADLINE)
        .expect("stream poll cleanup deadline is nonzero");
    let prepared = PreparedExternalOperation::quarantined_blocking(
        kind,
        Box::new(StreamPollDecoder),
        bound,
        move || {
            std::thread::sleep(std::time::Duration::from_millis(STREAM_POLL_INTERVAL_MS));
            Ok(Box::new(()) as SendPayload)
        },
    );
    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation: Box::new(StreamPollContinuation { token }),
    }))
}

pub(super) fn stream_next(token: u64) -> sema_core::runtime::NativeResult {
    #[allow(unused_imports)]
    use sema_core::runtime::NativeOutcome;

    enum Pre {
        Err(SemaError),
        Done,
        Run { prefilled: bool },
    }
    let pre = STREAM_RUNS.with(|r| {
        let mut slab = r.borrow_mut();
        let st = slab
            .get_mut(&token)
            .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
        if let Some(error) = st.pending_error.take() {
            // The deltas that preceded this failure were delivered last batch;
            // the run is over — drop the entry and surface.
            slab.remove(&token);
            return Ok(Pre::Err(error));
        }
        if st.done {
            return Ok(Pre::Done);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let prefilled = st.dispatch.is_none() && st.rx.is_none();
        #[cfg(target_arch = "wasm32")]
        let prefilled = st.rx.is_none();
        Ok::<_, SemaError>(Pre::Run { prefilled })
    })?;

    let prefilled = match pre {
        Pre::Err(error) => return Err(error),
        Pre::Done => return Ok(NativeOutcome::Return(stream_batch_map(Vec::new(), true))),
        Pre::Run { prefilled } => prefilled,
    };

    #[cfg(not(target_arch = "wasm32"))]
    if stream_dispatch_ready(token)? {
        return stream_next_runtime_step(token);
    }

    // Pre-filled runs resolve without parking because there is nothing to overlap;
    // outside a runtime quantum, fall back to a blocking drain.
    if prefilled || !sema_core::in_runtime_quantum() {
        loop {
            if let Some(v) = stream_poll_batch(token, true)? {
                return Ok(NativeOutcome::Return(v));
            }
        }
    }

    // A unified-runtime quantum re-scans cooperatively via a short off-VM-thread
    // sleep (a bounded External wait), so sibling tasks overlap between delta
    // batches. (Wasm has no off-thread executor; the non-runtime blocking drain
    // above already handled `!in_runtime_quantum`, so drain synchronously here.)
    #[cfg(not(target_arch = "wasm32"))]
    {
        stream_next_runtime_step(token)
    }
    #[cfg(target_arch = "wasm32")]
    {
        loop {
            if let Some(v) = stream_poll_batch(token, true)? {
                return Ok(NativeOutcome::Return(v));
            }
        }
    }
}

/// `__stream-finish(token) → content-string`. Cleans the slab entry and returns
/// the assembled content (usage was already accounted exactly once when the
/// VM-thread finalizer handled `Done`).
pub(super) fn stream_finish(token: u64) -> Result<Value, SemaError> {
    let mut st = STREAM_RUNS
        .with(|r| r.borrow_mut().remove(&token))
        .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
    if let Some(error) = st.pending_error.take() {
        return Err(error);
    }
    let resp = st
        .response
        .take()
        .ok_or_else(|| SemaError::Llm("stream not finished".to_string()))?;
    Ok(Value::string(&resp.content))
}

pub(super) fn register(env: &Env) {
    // (llm/stream "prompt" callback {:max-tokens 200})
    // (llm/stream "prompt" {:max-tokens 200})  — prints to stdout
    //
    // The synchronous stream native. The public `llm/stream` is a prelude wrapper
    // that dispatches here outside a scheduler task and to the non-blocking
    // `__stream-begin`/`__stream-next` machinery inside one (siblings interleave
    // between delta batches there).
    register_fn_ctx(env, "__llm-stream-blocking", |ctx, args| {
        let (request, callback, opts_map) = parse_stream_args(args)?;
        if sema_core::in_runtime_quantum() {
            return Err(SemaError::eval(
                "__llm-stream-blocking cannot run inside the cooperative runtime",
            )
            .with_hint("call llm/stream so its provider and callback can suspend cooperatively"));
        }
        let conv_scope = ConvScope::from_opts(opts_map.as_ref());

        // Streaming bypasses do_complete/track_usage, so it gets its own CLIENT span +
        // conversation scope. A caller-supplied id wins; else generate a fresh one (only
        // if no scope is already active).
        let _conv = conv_scope.open().or_else(|| {
            (sema_otel::current_conversation_id().is_none()).then(|| {
                sema_otel::set_conversation_scope(&sema_otel::new_conversation_id(), None, None)
            })
        });
        let span = sema_otel::llm_span("chat");
        span.set_request(
            request.temperature,
            request.max_tokens,
            &request.stop_sequences,
            None,
        );
        span.set_output_type(false);
        // Per-call observability tags/metadata (streaming bypasses do_complete).
        if let Some(ref m) = opts_map {
            let tags = get_opt_string_list(m, "tags");
            if !tags.is_empty() {
                span.set_tags(&tags);
            }
            let meta = get_opt_str_map(m, "metadata");
            if !meta.is_empty() {
                span.set_metadata(&meta);
            }
        }

        // Deliver each chunk to the user callback (or stdout). One callback for both the
        // model-pinned and default-model paths; the dispatch helper resolves the model.
        let mut chunk_cb = |chunk: &str| -> Result<(), crate::types::LlmError> {
            if let Some(ref cb) = callback {
                sema_core::call_callback(ctx, cb, &[Value::string(chunk)])
                    .map_err(|e| crate::types::LlmError::Config(e.to_string()))?;
            } else {
                print!("{}", chunk);
                use std::io::Write;
                std::io::stdout().flush().ok();
            }
            Ok(())
        };
        // Stream-open dispatch: budget pre-gate + rate-limit + fallback-at-open.
        let response = stream_with_dispatch(request, &mut chunk_cb, &span)?;

        // Print newline after streaming if using default display
        if callback.is_none() {
            println!();
        }

        track_usage(&response.usage)?;
        Ok(Value::string(&response.content))
    });

    // Non-blocking streaming natives (the `__stream-drive` prelude loop's
    // primitives; same bytecode-driven shape as the `__agent-*` loop above).
    register_fn(env, "__stream-begin", |args| {
        let (request, _callback, opts_map) = parse_stream_args(args)?;
        let conv_scope = ConvScope::from_opts(opts_map.as_ref());
        // Same scope rule as the blocking native: a caller-supplied id wins;
        // else generate a fresh one only if no scope is already active. The
        // DETACHED span captures the conversation id at creation, so the guard
        // need only live across span creation.
        let _conv = conv_scope.open().or_else(|| {
            (sema_otel::current_conversation_id().is_none()).then(|| {
                sema_otel::set_conversation_scope(&sema_otel::new_conversation_id(), None, None)
            })
        });
        let span = sema_otel::llm_span_detached("chat");
        span.set_request(
            request.temperature,
            request.max_tokens,
            &request.stop_sequences,
            None,
        );
        span.set_output_type(false);
        if let Some(ref m) = opts_map {
            let tags = get_opt_string_list(m, "tags");
            if !tags.is_empty() {
                span.set_tags(&tags);
            }
            let meta = get_opt_str_map(m, "metadata");
            if !meta.is_empty() {
                span.set_metadata(&meta);
            }
        }
        stream_run_begin(request, span)
    });
    register_runtime_fn_ctx(env, "__stream-next", |_ctx, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("__stream-next", "1", args.len()));
        }
        stream_next(stream_token_arg(&args[0])?)
    });
    register_fn(env, "__stream-finish", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("__stream-finish", "1", args.len()));
        }
        stream_finish(stream_token_arg(&args[0])?)
    });
}
