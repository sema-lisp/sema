use super::*;

/// One entry in an `llm/with-fallback` chain: a provider name plus an optional
/// per-provider model override. When `model` is `Some`, that model id is used for
/// this provider regardless of any model pinned in the call body (chain override
/// wins) — this lets a chain target a different model per provider, e.g. Opus on
/// Anthropic but a GPT model on OpenAI. When `None`, the provider's configured
/// default model is used.
#[derive(Debug, Clone)]
pub(super) struct FallbackEntry {
    pub(super) provider: String,
    pub(super) model: Option<String>,
}

pub(super) fn rate_limit_last_value() -> u64 {
    RATE_LIMIT_LAST.with(|last| last.borrow().as_ref().map_or(0, |slot| slot.get()))
}

pub(super) fn set_rate_limit_last_value(value: u64) {
    RATE_LIMIT_LAST.with(|last| {
        let mut last = last.borrow_mut();
        if let Some(slot) = last.as_ref() {
            slot.set(value);
        } else {
            *last = Some(Rc::new(Cell::new(value)));
        }
    });
}

pub(super) fn do_complete(mut request: ChatRequest) -> Result<ChatResponse, SemaError> {
    apply_input_policy_to_request(&mut request)?;
    // Standalone completions get their own conversation id so every chat span carries
    // gen_ai.conversation.id; agent-nested completions inherit the agent's scope.
    let _conv = if sema_otel::current_conversation_id().is_none() {
        Some(sema_otel::set_conversation_scope(
            &sema_otel::new_conversation_id(),
            None,
            None,
        ))
    } else {
        None
    };
    // One CLIENT span per completion. Started here (before cache lookup) so a cache
    // hit still gets a span; request attrs are known up front, provider/model/usage
    // are filled in deeper where they're resolved.
    let span = sema_otel::llm_span("chat");
    span.set_request(
        request.temperature,
        request.max_tokens,
        &request.stop_sequences,
        request.reasoning_effort.as_deref(),
    );
    span.set_output_type(request.json_mode);
    // Advertise the tools available this turn (compat: OpenInference llm.tools.*,
    // Traceloop llm.request.functions.*). Only built when a backend compat is active.
    if sema_otel::compat_active() && !request.tools.is_empty() {
        let views: Vec<sema_otel::ToolView> = request
            .tools
            .iter()
            .map(|t| sema_otel::ToolView {
                name: t.name.clone(),
                description: t.description.clone(),
                json_schema: t.parameters.to_string(),
            })
            .collect();
        span.set_tools(&views);
    }
    // User :tags / :metadata for this call (auto-tags are derived inside the span).
    apply_call_telemetry_llm(&span);
    // Reset the serving-provider stamp so a cache hit (which serves no provider) doesn't
    // inherit a stale name from a prior completion.
    LAST_SERVING_PROVIDER.with(|p| *p.borrow_mut() = None);
    let cache_enabled = CACHE_ENABLED.with(|c| c.get());
    if !cache_enabled {
        return run_completion(request, &span);
    }
    // Keep request.model unchanged so fallback entries can apply their own model.
    let key_model = if request.model.is_empty() {
        primary_model_for_cache()?
    } else {
        request.model.clone()
    };
    let mut key_request = request.clone();
    key_request.model = key_model;
    let cache_key = compute_cache_key(&key_request);
    if let Some(cached) = load_cached(&cache_key) {
        if is_cache_valid(&cached) {
            enforce_stored_model_policy(&cached.provider, &cached.model, PolicySource::Cache)?;
            CACHE_HITS.with(|c| c.set(c.get() + 1));
            // A cache hit makes no provider call: no tokens are consumed and no money
            // is spent. Report ZERO usage so the caller's `track_usage` does not
            // re-charge session cost or burn the budget for a cached response.
            let mut resp = cache_hit_response(cached, key_request.model.clone());
            apply_output_policy_to_response(&mut resp, PolicySource::Cache)?;
            // Cache-hit span: no provider served it; tag gen_ai.cache.hit=true with
            // zero usage (matches the zero-usage accounting invariant).
            span.set_dispatch("", &resp.model);
            span.set_response(&response_facts("", &resp));
            return Ok(resp);
        }
    }
    CACHE_MISSES.with(|c| c.set(c.get() + 1));
    let response = run_completion(request, &span)?;
    let serving_provider = LAST_SERVING_PROVIDER.with(|p| p.borrow().clone().unwrap_or_default());
    store_cached(&cache_key, &response, &serving_provider);
    Ok(response)
}

/// Streaming counterpart of [`do_complete`] for the agent tool loop's `:on-text`
/// option. Opens the same per-completion `chat` span/scope, but drives
/// `stream_with_dispatch` and delivers each text delta to the Sema `on_text`
/// callback. Returns the assembled [`ChatResponse`] so the loop's tool-call
/// handling and `track_usage` accounting match the non-streaming path. Streaming
/// bypasses the completion cache (like `llm/stream`).
pub(super) fn do_complete_streaming(
    ctx: &EvalContext,
    request: ChatRequest,
    on_text: &Value,
) -> Result<ChatResponse, SemaError> {
    let _conv = if sema_otel::current_conversation_id().is_none() {
        Some(sema_otel::set_conversation_scope(
            &sema_otel::new_conversation_id(),
            None,
            None,
        ))
    } else {
        None
    };
    let span = sema_otel::llm_span("chat");
    span.set_request(
        request.temperature,
        request.max_tokens,
        &request.stop_sequences,
        request.reasoning_effort.as_deref(),
    );
    span.set_output_type(request.json_mode);
    let mut chunk_cb = |chunk: &str| -> Result<(), crate::types::LlmError> {
        sema_core::call_callback(ctx, on_text, &[Value::string(chunk)])
            .map_err(|e| crate::types::LlmError::Config(e.to_string()))?;
        Ok(())
    };
    stream_with_dispatch(request, &mut chunk_cb, &span)
}

/// Shapes a completed `ChatResponse` into the next native-runtime outcome on the
/// VM thread, after `track_usage`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) type CompleteFinalizeCallback =
    Box<dyn FnOnce(ChatResponse) -> Result<Value, SemaError>>;

#[cfg(not(target_arch = "wasm32"))]
pub(super) trait CompleteResponseContinuation: sema_core::runtime::Trace {
    fn finish(self: Box<Self>, response: ChatResponse) -> sema_core::runtime::NativeResult;
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) enum CompleteFinalize {
    /// A value-producing finalizer. Closures are opaque to the cycle collector,
    /// so every captured `Value` must also appear in `retained_values`.
    Value {
        callback: CompleteFinalizeCallback,
        retained_values: Vec<Value>,
    },
    /// A structural continuation that can return another call or suspension.
    /// Its implementation owns and traces all retained runtime state.
    Runtime(Box<dyn CompleteResponseContinuation>),
}

#[cfg(not(target_arch = "wasm32"))]
impl CompleteFinalize {
    pub(super) fn new(
        callback: impl FnOnce(ChatResponse) -> Result<Value, SemaError> + 'static,
    ) -> Self {
        Self::with_values(callback, Vec::new())
    }

    pub(super) fn with_values(
        callback: impl FnOnce(ChatResponse) -> Result<Value, SemaError> + 'static,
        retained_values: Vec<Value>,
    ) -> Self {
        Self::Value {
            callback: Box::new(callback),
            retained_values,
        }
    }

    pub(super) fn runtime(continuation: Box<dyn CompleteResponseContinuation>) -> Self {
        Self::Runtime(continuation)
    }

    fn finish(self, response: ChatResponse) -> sema_core::runtime::NativeResult {
        match self {
            Self::Value { callback, .. } => {
                callback(response).map(sema_core::runtime::NativeOutcome::Return)
            }
            Self::Runtime(continuation) => continuation.finish(response),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for CompleteFinalize {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        match self {
            Self::Value {
                retained_values, ..
            } => {
                for value in retained_values {
                    sink(sema_core::cycle::GcEdge::Value(value));
                }
                true
            }
            Self::Runtime(continuation) => continuation.trace(sink),
        }
    }
}

/// On-VM-thread preparation result for a runtime completion. `Inline` is a cache
/// hit or cassette replay that made no provider call; the caller accounts zero
/// usage and finalizes without suspending. `Offload` carries the provider plan.
#[cfg(not(target_arch = "wasm32"))]
pub(super) enum CompletePrep {
    Inline(ChatResponse),
    Offload(Box<CompleteOffloadPlan>),
}

/// Everything the provider-at-a-time runtime driver and its VM-thread finalizer
/// need. The chain is resolved on the VM thread so native workers touch no
/// thread-locals; `request_for_messages` is retained for the span's I/O attributes.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct CompleteOffloadPlan {
    pub(super) chain: Vec<ResolvedProvider>,
    /// Model `:skip` applies to every explicit fallback chain, including one entry.
    pub(super) explicit_fallback: bool,
    pub(super) request: ChatRequest,
    pub(super) max_retries: u32,
    pub(super) retry_base_ms: u64,
    pub(super) rate_limit_wait_ms: u64,
    pub(super) span: sema_otel::LlmSpan,
    pub(super) cache_key: Option<String>,
    pub(super) cassette_record_key: Option<String>,
    pub(super) cassette_scope: Option<CassetteScope>,
    pub(super) request_for_messages: ChatRequest,
}

/// The on-VM-thread prep stage of a runtime completion: open the conversation
/// scope, start the DETACHED `chat` span, consult the response cache and cassette
/// (either can short-circuit to `Inline`), then resolve the fallback chain +
/// rate-limit/retry parameters into `Arc` clones each native attempt can own. All
/// runtime completion paths use this stage to keep cache, cassette, and retry
/// behavior aligned.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn complete_offload_prep(mut request: ChatRequest) -> Result<CompletePrep, SemaError> {
    apply_input_policy_to_request(&mut request)?;
    // Standalone completions get their own conversation scope (so the chat span
    // carries gen_ai.conversation.id); agent-nested ones inherit. The detached span
    // captures the conversation id at creation, so the guard need only live across
    // span creation below.
    let _conv = if sema_otel::current_conversation_id().is_none() {
        Some(sema_otel::set_conversation_scope(
            &sema_otel::new_conversation_id(),
            None,
            None,
        ))
    } else {
        None
    };

    // DETACHED chat span: parent captured now, finalized when the offload lands
    // (when the active-span stack may hold a sibling task's span, so the span must
    // not pop the stack on drop).
    let span = sema_otel::llm_span_detached("chat");
    span.set_request(
        request.temperature,
        request.max_tokens,
        &request.stop_sequences,
        request.reasoning_effort.as_deref(),
    );
    span.set_output_type(request.json_mode);
    if sema_otel::compat_active() && !request.tools.is_empty() {
        let views: Vec<sema_otel::ToolView> = request
            .tools
            .iter()
            .map(|t| sema_otel::ToolView {
                name: t.name.clone(),
                description: t.description.clone(),
                json_schema: t.parameters.to_string(),
            })
            .collect();
        span.set_tools(&views);
    }
    apply_call_telemetry_llm(&span);
    // ── Cache lookup (hit → Inline, zero usage) ──────────────────────────
    let cache_enabled = CACHE_ENABLED.with(|c| c.get());
    let cache_key = if cache_enabled {
        let key_model = if request.model.is_empty() {
            primary_model_for_cache()?
        } else {
            request.model.clone()
        };
        let mut key_request = request.clone();
        key_request.model = key_model;
        let key = compute_cache_key(&key_request);
        // MEM-only probe on the VM thread — a mem hit short-circuits with ZERO usage
        // exactly like `do_complete`. The DISK read is offloaded by the driver's
        // cache-peek phase (so no fs touches the quantum), where the hit/miss counters
        // are bumped once the disk result lands.
        if let Some(cached) = load_cached_mem(&key) {
            if is_cache_valid(&cached) {
                enforce_stored_model_policy(&cached.provider, &cached.model, PolicySource::Cache)?;
                CACHE_HITS.with(|c| c.set(c.get() + 1));
                let mut resp = cache_hit_response(cached, key_request.model.clone());
                apply_output_policy_to_response(&mut resp, PolicySource::Cache)?;
                set_guarded_response_telemetry(&span, &request, "", &resp);
                drop(span);
                return Ok(CompletePrep::Inline(resp));
            }
        }
        Some(key)
    } else {
        None
    };

    // ── Cassette decision (replay → Inline; miss → Err) ──────────────────
    // Keyed by the request as-is (no default-model resolution), matching
    // `run_completion`'s key so record/replay agree with the sync path.
    let cassette_scope = current_cassette_scope();
    let cassette_decision = cassette_scope.as_ref().map(|scope| {
        let key = compute_cache_key(&request);
        (key.clone(), scope.borrow().decide(&key))
    });
    match cassette_decision {
        Some((_, crate::cassette::Decision::Replay(entry))) => {
            enforce_stored_model_policy(&entry.provider, &entry.model, PolicySource::Cassette)?;
            let mut resp = entry.to_response();
            apply_output_policy_to_response(&mut resp, PolicySource::Cassette)?;
            set_guarded_response_telemetry(&span, &request, "cassette", &resp);
            drop(span);
            return Ok(CompletePrep::Inline(resp));
        }
        Some((k, crate::cassette::Decision::Miss(_))) => return Err(cassette_miss_error(&k)),
        _ => {}
    }
    let cassette_record_key = match cassette_decision {
        Some((k, crate::cassette::Decision::Record)) => Some(k),
        _ => None,
    };

    // ── Resolve the fallback chain (or default provider) into Arc clones ──
    // Done on the VM thread so the offloaded worker touches no thread-locals.
    // Reserve this call's rate-limit slot HERE, synchronously. Runtime calls
    // spend the wait as a structural timer before the provider driver starts;
    // host calls use the synchronous path's existing pacing behavior.
    let rate_limit_wait_ms = reserve_rate_limit_wait_ms();
    let max_retries = NETWORK_MAX_RETRIES.with(|c| c.get());
    // Capture the retry-backoff base on the VM thread so each native provider
    // attempt honors it (pool workers have their own RETRY_BASE_MS TLS copies).
    let retry_base_ms = RETRY_BASE_MS.with(|c| c.get());
    let (chain, explicit_fallback) = resolve_stream_chain()?;

    let request_for_messages = request.clone();
    Ok(CompletePrep::Offload(Box::new(CompleteOffloadPlan {
        chain,
        explicit_fallback,
        request,
        max_retries,
        retry_base_ms,
        rate_limit_wait_ms,
        span,
        cache_key,
        cassette_record_key,
        cassette_scope,
        request_for_messages,
    })))
}

/// VM-thread finalizer for a successful offloaded completion: finalize the span (retry spans,
/// dispatch/response/messages facts), store the cache entry, record the cassette,
/// fold the leaf usage accumulator, then `track_usage` under the captured budget
/// frame (a budget overrun fails the task, exactly as the sync path's `?`) and
/// finally `finalize` the response into the per-native return value. `span`,
/// `finalize`, and the captured `Rc` slots are all consumed exactly once here.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
pub(super) fn finalize_complete_success(
    outcome: CompleteOutcome,
    span: sema_otel::LlmSpan,
    cache_key: Option<String>,
    cassette_record_key: Option<String>,
    cassette_scope: Option<CassetteScope>,
    usage_accum_slot: Option<Rc<RefCell<LeafUsage>>>,
    budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
    request_for_messages: &ChatRequest,
    finalize: CompleteFinalize,
) -> sema_core::runtime::NativeResult {
    let CompleteOutcome {
        mut resp,
        serving_provider,
        serving_model,
        retry_events,
    } = outcome;
    // Emit retry spans + set the response facts UNDER this span so children parent
    // correctly (the detached span is not on the stack). `entered` installs it as
    // the active parent for the closure, then restores.
    span.entered(|| {
        emit_retry_spans(&retry_events);
    });
    span.set_dispatch(&serving_provider, &serving_model);
    if let Err(error) = apply_output_policy_to_response(&mut resp, PolicySource::Request) {
        span.record_error("policy", &error.to_string());
        drop(span);
        account_complete_usage(
            &serving_provider,
            &resp.usage,
            usage_accum_slot.as_ref(),
            budget_slot,
        )?;
        return Err(error);
    }
    span.set_response(&response_facts(&serving_provider, &resp));
    span.set_messages(
        &messages_json(&request_for_messages.messages),
        &content_json("assistant", &resp.content),
        request_for_messages
            .system
            .as_deref()
            .map(|s| content_json("system", s))
            .as_deref(),
    );
    drop(span); // ends the span
    set_serving_provider(&serving_provider);
    if let Some(key) = &cache_key {
        store_cached(key, &resp, &serving_provider);
    }
    if let Some(key) = &cassette_record_key {
        cassette_scope_record(
            &cassette_scope,
            crate::cassette::TapeEntry::from_response(key, &serving_provider, &resp),
        );
    }
    account_complete_usage(
        &serving_provider,
        &resp.usage,
        usage_accum_slot.as_ref(),
        budget_slot,
    )?;
    finalize.finish(resp)
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn account_complete_usage(
    serving_provider: &str,
    usage: &Usage,
    usage_accum_slot: Option<&Rc<RefCell<LeafUsage>>>,
    budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
) -> Result<(), SemaError> {
    // Fold this completion into the LEAF'S OWN captured accumulator frame — the
    // `Rc` snapshotted at dispatch, not whatever scope is active when the offload
    // lands. Suppress `track_usage`'s ambient accumulator fold so the completion
    // is counted exactly once.
    if let Some(slot) = usage_accum_slot {
        let cost = pricing::calculate_cost_for(serving_provider, usage);
        accumulate_into(slot, usage, cost);
    }
    let previous_budget =
        ACTIVE_BUDGET.with(|active| std::mem::replace(&mut *active.borrow_mut(), budget_slot));
    let result = USAGE_ACCUM_SUPPRESS.with(|suppress| {
        suppress.set(true);
        let result = track_usage(usage);
        suppress.set(false);
        result
    });
    ACTIVE_BUDGET.with(|active| *active.borrow_mut() = previous_budget);
    result
}

/// Completion-kind tag for an agent/chat provider round offloaded through the
/// unified runtime's External-wait machinery (distinct from mcp's kinds).
#[cfg(not(target_arch = "wasm32"))]
pub(super) const AGENT_COMPLETE_COMPLETION_KIND: u64 = 0x6c6c_6d63; // "llmc"

/// Completion-kind tag for the offloaded cache disk-read peek.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const CACHE_PEEK_COMPLETION_KIND: u64 = 0x6361_6368; // "cach"

/// Completion-kind tag for an offloaded cassette tape LOAD (disk read).
#[cfg(not(target_arch = "wasm32"))]
pub(super) const CASSETTE_LOAD_COMPLETION_KIND: u64 = 0x6373_6c64; // "csld"

/// True when a provider call must offload and suspend on the unified runtime's
/// External wait. Root-main and spawned tasks obey the same boundary.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn in_runtime_offload_task() -> bool {
    in_runtime_offload_context()
}

/// Route a prepared completion through the offload appropriate to the current
/// execution context, returning a `NativeResult`. This is the shared split every
/// simple completion entry point (`llm/complete`, `llm/send`, `llm/chat` no-tools,
/// `llm/compare`) uses so runtime and synchronous paths stay aligned:
///
/// * any unified-runtime quantum → [`do_complete_runtime_suspend`]; Sema callbacks
///   run as structural calls and native attempts park on External;
/// * a host call → the synchronous provider call.
///
/// `finalize` shapes the return value from the `ChatResponse` and runs on the VM
/// thread after `track_usage`, so it MUST be equivalent across both paths.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn dispatch_complete_offload(
    request: ChatRequest,
    finalize: CompleteFinalize,
) -> sema_core::runtime::NativeResult {
    if in_runtime_offload_task() {
        return do_complete_runtime_suspend(request, finalize);
    }
    let response = do_complete(request)?;
    track_usage(&response.usage)?;
    finalize.finish(response)
}

/// Cooperative provider-complete round under the unified runtime. It uses the
/// shared on-VM-thread preparation and finalization stages, then drives providers
/// in fallback order. Sema callbacks are structural VM calls; each native attempt
/// runs on the executor's IO pool behind a cancellable External wait. A cache hit
/// or cassette replay finalizes inline. Returns the `NativeOutcome` directly on the
/// runtime native ABI (`__agent-step` drives it through its runtime callback).
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn do_complete_runtime_suspend(
    request: ChatRequest,
    finalize: CompleteFinalize,
) -> sema_core::runtime::NativeResult {
    let plan = match complete_offload_prep(request)? {
        CompletePrep::Inline(resp) => {
            track_usage(&resp.usage)?;
            return finalize.finish(resp);
        }
        CompletePrep::Offload(plan) => *plan,
    };
    dispatch_complete_runtime_plan(plan, finalize)
}

/// Drive a runtime completion one provider at a time. Sema-defined providers run
/// as structural VM calls, while native providers suspend on one external attempt
/// (including that provider's retry loop). Owning the whole chain in one
/// continuation preserves arbitrary fallback order without blocking a quantum.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn dispatch_complete_runtime_plan(
    plan: CompleteOffloadPlan,
    finalize: CompleteFinalize,
) -> sema_core::runtime::NativeResult {
    Box::new(RuntimeCompleteDriver {
        plan,
        finalize,
        next_provider: 0,
        last_error: None,
        phase: RuntimeCompletePhase::Ready,
        cache_peeked: false,
        usage_accum_slot: current_usage_accum(),
        budget_slot: active_budget(),
    })
    .advance()
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) type CompleteAttemptSlot =
    Rc<RefCell<Option<Result<CompleteOutcome, crate::types::LlmError>>>>;

/// Delivery slot for the offloaded cache-peek disk read. Outer `Option` = "the
/// decoder ran"; inner `Option<CachedResponse>` = "a well-formed on-disk entry" (or a
/// miss). Validity/TTL is checked on the VM thread when the read lands.
#[cfg(not(target_arch = "wasm32"))]
pub(super) type CachePeekSlot = Rc<RefCell<Option<Option<CachedResponse>>>>;

#[cfg(not(target_arch = "wasm32"))]
pub(super) enum RuntimeCompletePhase {
    Ready,
    /// Parked on the offloaded cache disk read (blocking tier); a hit finalizes inline
    /// with zero usage, a miss proceeds to the provider chain.
    CachePeek {
        slot: CachePeekSlot,
    },
    Pacing,
    Sema {
        provider: String,
        model: String,
    },
    Native {
        provider: String,
        slot: CompleteAttemptSlot,
    },
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct RuntimeCompleteDriver {
    pub(super) plan: CompleteOffloadPlan,
    pub(super) finalize: CompleteFinalize,
    pub(super) next_provider: usize,
    pub(super) last_error: Option<crate::types::LlmError>,
    pub(super) phase: RuntimeCompletePhase,
    /// Whether the offloaded cache disk peek has already run (it runs at most once,
    /// before pacing and the provider chain).
    pub(super) cache_peeked: bool,
    pub(super) usage_accum_slot: Option<Rc<RefCell<LeafUsage>>>,
    pub(super) budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for RuntimeCompleteDriver {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sema_core::runtime::Trace::trace(&self.finalize, sink)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeCompleteDriver {
    fn advance(mut self: Box<Self>) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeCall, NativeOutcome, NativeSuspend, WaitKind};

        // ── Cache disk peek (once, before pacing/providers) ──────────────────
        // The on-disk read is offloaded to the blocking tier so the quantum never
        // touches the filesystem; a hit finalizes inline with ZERO usage and leaves
        // any sibling task runnable while parked.
        if !self.cache_peeked {
            self.cache_peeked = true;
            if let Some(key) = self.plan.cache_key.clone() {
                return self.suspend_cache_peek(key);
            }
        }

        if self.plan.rate_limit_wait_ms > 0 {
            let delay = std::time::Duration::from_millis(self.plan.rate_limit_wait_ms);
            self.plan.rate_limit_wait_ms = 0;
            self.phase = RuntimeCompletePhase::Pacing;
            return Ok(NativeOutcome::Suspend(NativeSuspend {
                wait: WaitKind::Timer(delay),
                continuation: self,
            }));
        }

        loop {
            let Some(entry) = self.plan.chain.get(self.next_provider) else {
                let error = self.last_error.take().unwrap_or_else(|| {
                    crate::types::LlmError::Config("all providers failed".to_string())
                });
                self.plan
                    .span
                    .record_error("provider_error", &error.to_string());
                return Err(SemaError::Llm(error.to_string()));
            };
            self.next_provider += 1;

            let provider = entry.provider.clone();
            let provider_name = entry.name.clone();
            let mut request = self.plan.request.clone();
            if let Some(model) = &entry.model {
                request.model = model.clone();
            } else if request.model.is_empty() {
                request.model = provider.default_model().to_string();
            }
            if !model_target_allowed(
                &provider_name,
                &request.model,
                PolicySource::Request,
                self.plan.explicit_fallback,
            )? {
                continue;
            }

            if provider.runs_on_vm_thread() {
                let callback = match lisp_provider_complete_callback(&provider_name) {
                    Ok(callback) => callback,
                    Err(error) => {
                        eprintln!(
                            "Provider '{}' failed: {error}, trying next...",
                            provider_name
                        );
                        self.last_error = Some(error);
                        continue;
                    }
                };
                let model = request.model.clone();
                self.phase = RuntimeCompletePhase::Sema {
                    provider: provider_name,
                    model,
                };
                return Ok(NativeOutcome::Call(NativeCall {
                    callable: callback,
                    args: vec![chat_request_to_value(&request)],
                    continuation: self,
                }));
            }

            return self.suspend_native_attempt(provider, provider_name, request);
        }
    }

    /// Suspend on the offloaded cache disk read for `key`. The read runs on the
    /// blocking tier (`interruptible_blocking`), never the VM quantum.
    fn suspend_cache_peek(mut self: Box<Self>, key: String) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{
            CompletionKind, InterruptibleResource, NativeOutcome, NativeSuspend,
            PreparedExternalOperation, SendPayload, WaitKind,
        };
        let slot: CachePeekSlot = Rc::new(RefCell::new(None));
        let decoder = Box::new(CachePeekDecoder {
            slot: Rc::clone(&slot),
        });
        self.phase = RuntimeCompletePhase::CachePeek { slot };
        let kind = CompletionKind::try_from_raw(CACHE_PEEK_COMPLETION_KIND)
            .expect("cache-peek completion kind is nonzero");
        let resource =
            InterruptibleResource::new("llm/cache-peek", Box::new(CompleteNoopCancelHook));
        let path = cache_file_path(&key);
        let prepared =
            PreparedExternalOperation::interruptible_blocking(kind, decoder, resource, move || {
                Ok(Box::new(read_cached_from_disk(&path)) as SendPayload)
            });
        Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::External(Box::new(prepared)),
            continuation: self,
        }))
    }

    /// Resume after the offloaded cache disk read. A valid on-disk entry is a HIT:
    /// populate the in-memory cache, count the hit, and finalize inline with ZERO
    /// usage — identical to the mem hit, so `track_usage` never recharges. Otherwise
    /// count the miss and proceed to the provider chain.
    fn finish_cache_peek(
        mut self: Box<Self>,
        disk: Option<CachedResponse>,
    ) -> sema_core::runtime::NativeResult {
        if let Some(cached) = disk {
            if is_cache_valid(&cached) {
                enforce_stored_model_policy(&cached.provider, &cached.model, PolicySource::Cache)?;
                CACHE_HITS.with(|c| c.set(c.get() + 1));
                let Self { plan, finalize, .. } = *self;
                if let Some(key) = &plan.cache_key {
                    CACHE_MEM.with(|c| c.borrow_mut().insert(key.clone(), cached.clone()));
                }
                let usage_model = cached.model.clone();
                let mut resp = cache_hit_response(cached, usage_model);
                apply_output_policy_to_response(&mut resp, PolicySource::Cache)?;
                set_guarded_response_telemetry(&plan.span, &plan.request_for_messages, "", &resp);
                drop(plan.span);
                track_usage(&resp.usage)?;
                return finalize.finish(resp);
            }
        }
        // Miss (no entry, or entry present but invalid) — count it once here (deferred
        // from prep) so `(llm/cache-stats)` :misses is accurate, then run providers.
        CACHE_MISSES.with(|c| c.set(c.get() + 1));
        self.phase = RuntimeCompletePhase::Ready;
        self.advance()
    }

    fn suspend_native_attempt(
        mut self: Box<Self>,
        provider: std::sync::Arc<dyn LlmProvider>,
        provider_name: String,
        request: ChatRequest,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{
            CompletionKind, InterruptibleResource, NativeOutcome, NativeSuspend,
            PreparedExternalOperation, SendPayload, WaitKind,
        };

        let max_retries = self.plan.max_retries;
        let retry_base_ms = self.plan.retry_base_ms;
        let serving_model = request.model.clone();
        let slot = Rc::new(RefCell::new(None));
        let decoder = Box::new(CompleteAttemptDecoder {
            slot: Rc::clone(&slot),
        });
        self.phase = RuntimeCompletePhase::Native {
            provider: provider_name.clone(),
            slot,
        };

        let kind = CompletionKind::try_from_raw(AGENT_COMPLETE_COMPLETION_KIND)
            .expect("agent completion kind is nonzero");
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let resource = InterruptibleResource::new(
            "agent/complete",
            Box::new(LlmSelectCancelHook {
                signal: Some(cancel_tx),
            }),
        );
        let prepared = PreparedExternalOperation::interruptible_async(
            kind,
            decoder,
            resource,
            move || async move {
                let work = async move {
                    let _inflight = InflightGuard::new();
                    complete_with_retry_collecting_async(
                        &provider,
                        &request,
                        max_retries,
                        retry_base_ms,
                    )
                    .await
                    .map(|(resp, retry_events)| CompleteOutcome {
                        resp,
                        serving_provider: provider_name,
                        serving_model,
                        retry_events,
                    })
                };
                let result = tokio::select! {
                    biased;
                    _ = cancel_rx => {
                        Err(crate::types::LlmError::Config("cancelled".to_string()))
                    }
                    result = work => result,
                };
                Ok(Box::new(result) as SendPayload)
            },
        );
        Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::External(Box::new(prepared)),
            continuation: self,
        }))
    }

    fn provider_failed(
        mut self: Box<Self>,
        provider: String,
        error: crate::types::LlmError,
    ) -> sema_core::runtime::NativeResult {
        eprintln!("Provider '{provider}' failed: {error}, trying next...");
        self.last_error = Some(error);
        self.phase = RuntimeCompletePhase::Ready;
        self.advance()
    }

    fn finish(self: Box<Self>, outcome: CompleteOutcome) -> sema_core::runtime::NativeResult {
        let Self {
            plan,
            finalize,
            usage_accum_slot,
            budget_slot,
            ..
        } = *self;
        finalize_complete_success(
            outcome,
            plan.span,
            plan.cache_key,
            plan.cassette_record_key,
            plan.cassette_scope,
            usage_accum_slot,
            budget_slot,
            &plan.request_for_messages,
            finalize,
        )
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::NativeContinuation for RuntimeCompleteDriver {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;

        match (&self.phase, input) {
            (RuntimeCompletePhase::CachePeek { slot }, ResumeInput::Returned(_)) => {
                let disk = slot.borrow_mut().take().flatten();
                self.finish_cache_peek(disk)
            }
            (RuntimeCompletePhase::CachePeek { .. }, ResumeInput::Failed(_)) => {
                // A failed disk read is a cache miss, not a fatal error — run providers.
                self.finish_cache_peek(None)
            }
            (RuntimeCompletePhase::Pacing, ResumeInput::Returned(_)) => {
                let mut this = self;
                this.phase = RuntimeCompletePhase::Ready;
                this.advance()
            }
            (RuntimeCompletePhase::Sema { provider, model }, ResumeInput::Returned(value)) => {
                let provider = provider.clone();
                let model = model.clone();
                match parse_lisp_provider_response(&value, &model) {
                    Ok(resp) => self.finish(CompleteOutcome {
                        resp,
                        serving_provider: provider,
                        serving_model: model,
                        retry_events: Vec::new(),
                    }),
                    Err(error) => self.provider_failed(provider, error),
                }
            }
            (RuntimeCompletePhase::Sema { provider, .. }, ResumeInput::Failed(error)) => {
                let provider = provider.clone();
                self.provider_failed(
                    provider,
                    crate::types::LlmError::Api {
                        status: 0,
                        message: error.to_string(),
                    },
                )
            }
            (RuntimeCompletePhase::Native { provider, slot }, ResumeInput::Returned(_)) => {
                let provider = provider.clone();
                let result = slot
                    .borrow_mut()
                    .take()
                    .ok_or_else(|| SemaError::eval("agent completion result was not delivered"))?;
                match result {
                    Ok(outcome) => self.finish(outcome),
                    Err(error) => self.provider_failed(provider, error),
                }
            }
            (_, ResumeInput::Failed(error)) => {
                self.plan.span.record_error("io", &error.to_string());
                Err(error)
            }
            (_, ResumeInput::Cancelled(reason)) => Err(SemaError::eval(format!(
                "agent completion was cancelled ({reason:?})"
            ))),
            (_, ResumeInput::Runtime(_)) => Err(SemaError::eval(
                "agent completion driver received an unexpected runtime response",
            )),
            (RuntimeCompletePhase::Ready, ResumeInput::Returned(_)) => Err(SemaError::eval(
                "agent completion driver resumed without an active attempt",
            )),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct CompleteAttemptDecoder {
    pub(super) slot: CompleteAttemptSlot,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for CompleteAttemptDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::CompletionDecoder for CompleteAttemptDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> sema_core::runtime::DecodedCompletion {
        let payload = result.map_err(|failure| {
            SemaError::eval(format!("agent completion: {}", failure.message()))
        })?;
        let result = sema_core::runtime::downcast_send_payload::<
            Result<CompleteOutcome, crate::types::LlmError>,
        >(payload, "agent-complete")
        .map_err(|failure| SemaError::eval(format!("agent completion: {}", failure.message())))?;
        let mut slot = self.slot.borrow_mut();
        if slot.is_some() {
            return Err(SemaError::eval(
                "agent completion result was delivered more than once",
            ));
        }
        *slot = Some(result);
        Ok(Value::nil())
    }
}

/// Decoder for the offloaded cache disk peek: hands the on-disk `Option<CachedResponse>`
/// back to the driver. Holds no `Value`, so it emits no GC edges.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct CachePeekDecoder {
    slot: CachePeekSlot,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for CachePeekDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::CompletionDecoder for CachePeekDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> sema_core::runtime::DecodedCompletion {
        let payload = result
            .map_err(|failure| SemaError::eval(format!("cache peek: {}", failure.message())))?;
        let cached = sema_core::runtime::downcast_send_payload::<Option<CachedResponse>>(
            payload,
            "cache-peek",
        )
        .map_err(|failure| SemaError::eval(format!("cache peek: {}", failure.message())))?;
        *self.slot.borrow_mut() = Some(cached);
        Ok(Value::nil())
    }
}

/// What to do once an offloaded cassette tape LOAD lands.
#[cfg(not(target_arch = "wasm32"))]
pub(super) enum CassetteLoadThen {
    /// `llm/with-cassette`: install the loaded cassette scope, then CALL the body
    /// thunk under a scope-teardown continuation.
    WithBody(Value),
    /// `llm/cassette-load`: install the cassette in the ambient scope, then return nil.
    Install,
}

/// Continuation that reconstructs a cassette from an offloaded tape read and either
/// runs the `with-cassette` body or installs the ambient scope. Traces the body thunk
/// (a `Value` that may capture live state).
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct CassetteLoadContinuation {
    slot: Rc<RefCell<Option<crate::cassette::Tape>>>,
    path: std::path::PathBuf,
    mode: crate::cassette::CassetteMode,
    then: CassetteLoadThen,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for CassetteLoadContinuation {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        if let CassetteLoadThen::WithBody(body) = &self.then {
            sink(sema_core::cycle::GcEdge::Value(body));
        }
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::NativeContinuation for CassetteLoadContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeCall, NativeOutcome, ResumeInput};
        match input {
            ResumeInput::Returned(_) => {
                let Self {
                    slot,
                    path,
                    mode,
                    then,
                } = *self;
                let tape = slot
                    .borrow_mut()
                    .take()
                    .ok_or_else(|| SemaError::eval("cassette load result was not delivered"))?;
                let cassette = crate::cassette::Cassette::from_tape(path, mode, tape);
                match then {
                    CassetteLoadThen::Install => {
                        install_cassette(cassette);
                        Ok(NativeOutcome::Return(Value::nil()))
                    }
                    CassetteLoadThen::WithBody(body_fn) => {
                        let teardown = install_loaded_cassette(cassette);
                        Ok(NativeOutcome::Call(NativeCall {
                            callable: body_fn,
                            args: Vec::new(),
                            continuation: Box::new(ScopeGuardContinuation {
                                teardown: Some(teardown),
                            }),
                        }))
                    }
                }
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "llm/with-cassette load was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "cassette load received an unexpected runtime response",
            )),
        }
    }
}

/// Decoder for an offloaded cassette tape LOAD: hands the loaded `Tape` to the
/// continuation. Holds no `Value`, so it emits no GC edges.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct CassetteLoadDecoder {
    slot: Rc<RefCell<Option<crate::cassette::Tape>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for CassetteLoadDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::CompletionDecoder for CassetteLoadDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> sema_core::runtime::DecodedCompletion {
        let payload = result
            .map_err(|failure| SemaError::eval(format!("cassette load: {}", failure.message())))?;
        let tape = sema_core::runtime::downcast_send_payload::<crate::cassette::Tape>(
            payload,
            "cassette-load",
        )
        .map_err(|failure| SemaError::eval(format!("cassette load: {}", failure.message())))?;
        *self.slot.borrow_mut() = Some(tape);
        Ok(Value::nil())
    }
}

/// Suspend on an offloaded cassette tape LOAD (disk read) so the quantum never touches
/// the filesystem, resuming into `then`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn suspend_cassette_load(
    path: std::path::PathBuf,
    mode: crate::cassette::CassetteMode,
    then: CassetteLoadThen,
) -> sema_core::runtime::NativeResult {
    use sema_core::runtime::{
        CompletionKind, InterruptibleResource, NativeOutcome, NativeSuspend,
        PreparedExternalOperation, SendPayload, WaitKind,
    };
    let slot: Rc<RefCell<Option<crate::cassette::Tape>>> = Rc::new(RefCell::new(None));
    let decoder = Box::new(CassetteLoadDecoder {
        slot: Rc::clone(&slot),
    });
    let kind = CompletionKind::try_from_raw(CASSETTE_LOAD_COMPLETION_KIND)
        .expect("cassette load kind is nonzero");
    let resource =
        InterruptibleResource::new("llm/with-cassette", Box::new(CompleteNoopCancelHook));
    let load_path = path.clone();
    let prepared =
        PreparedExternalOperation::interruptible_blocking(kind, decoder, resource, move || {
            Ok(Box::new(crate::cassette::Tape::load(&load_path)) as SendPayload)
        });
    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation: Box::new(CassetteLoadContinuation {
            slot,
            path,
            mode,
            then,
        }),
    }))
}

/// No-op cancel hook for the completion External wait: the executor aborts the
/// offloaded future itself (dropping the in-flight request); there is no external
/// resource to tear down here.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct CompleteNoopCancelHook;

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for CompleteNoopCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::CancelHook for CompleteNoopCancelHook {
    fn cancel(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        Ok(sema_core::runtime::CancelDisposition::Reaped)
    }
    fn reap(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        Ok(sema_core::runtime::CancelDisposition::Reaped)
    }
}

/// Cancel hook for an interruptible async LLM offload whose in-flight request is
/// torn down by firing a one-shot signal that the job's `tokio::select!` awaits.
/// Firing it drops the in-flight provider future (closing the connection) — the
/// same drop-on-cancel the http path gives, and robust under an executor whose
/// async tier drives futures with `block_on` (no task-abort). Lives on the runtime
/// thread, so it need not be `Send`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct LlmSelectCancelHook {
    pub(super) signal: Option<tokio::sync::oneshot::Sender<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for LlmSelectCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::CancelHook for LlmSelectCancelHook {
    fn cancel(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        if let Some(signal) = self.signal.take() {
            // Err (receiver already gone) means the job finished first; nothing to
            // tear down. Either way the resource is reaped.
            let _ = signal.send(());
        }
        Ok(sema_core::runtime::CancelDisposition::Reaped)
    }
    fn reap(
        &mut self,
    ) -> Result<sema_core::runtime::CancelDisposition, sema_core::runtime::CancelHookError> {
        Ok(sema_core::runtime::CancelDisposition::Reaped)
    }
}

/// Generic pass-through continuation for a value-producing offload whose decoder
/// already built the final `Value`/error (`llm/embed`, `llm/rerank`, `llm/batch`,
/// `llm/io-sleep-once`). Holds no `Value`, so it emits no GC edges.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct OffloadValueContinuation {
    pub(super) op: &'static str,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for OffloadValueContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::NativeContinuation for OffloadValueContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeOutcome, ResumeInput};
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "{} was cancelled ({reason:?})",
                self.op
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
                "{} continuation received an unexpected runtime response",
                self.op
            ))),
        }
    }
}

/// Completion-kind tag for the `llm/io-sleep-once` spike leaf's External wait.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const IO_SLEEP_COMPLETION_KIND: u64 = 0x736c_7031; // "slp1"

/// Decoder for the `llm/io-sleep-once` spike leaf: the offloaded timer resolves to
/// the `id` (`i64`). Holds no state, emits no GC edges.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct IoSleepDecoder;

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for IoSleepDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::CompletionDecoder for IoSleepDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> sema_core::runtime::DecodedCompletion {
        match result {
            Ok(payload) => {
                match sema_core::runtime::downcast_send_payload::<i64>(payload, "io-sleep-once") {
                    Ok(id) => Ok(Value::int(id)),
                    Err(failure) => Err(SemaError::eval(failure.message().to_string())),
                }
            }
            Err(failure) => Err(SemaError::eval(format!(
                "io-sleep-once: {}",
                failure.message()
            ))),
        }
    }
}

/// Cassette interception seam: below the otel span + response cache (set up in
/// `do_complete`), above the real provider chain (`do_complete_inner`). When a
/// cassette is active it replays a recorded response (still emitting the chat
/// span, populated from the recorded model/usage) or records a fresh one; with no
/// cassette it's a transparent passthrough. See `crate::cassette`.
pub(super) fn run_completion(
    request: ChatRequest,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    if current_cassette_scope().is_none() {
        let mut response = do_complete_inner(request.clone(), span)?;
        if let Err(error) = apply_output_policy_to_response(&mut response, PolicySource::Request) {
            track_usage(&response.usage)?;
            return Err(error);
        }
        set_guarded_response_telemetry(span, &request, "", &response);
        return Ok(response);
    }
    // Key by the request as-is (no default-model resolution) so record and replay
    // produce the same key for an identical call, even with no provider configured
    // (keyless replay). Shares the hashing with the response cache.
    let key = compute_cache_key(&request);
    let decision = cassette_decide(&key).expect("cassette scope checked above");
    match decision {
        crate::cassette::Decision::Replay(entry) => {
            enforce_stored_model_policy(&entry.provider, &entry.model, PolicySource::Cassette)?;
            // A replayed call is a stand-in for a real one: emit the span with the
            // recorded facts and let the caller's usage/cost accounting run on the
            // recorded tokens (distinct from a cache hit, which reports zero usage).
            let mut response = entry.to_response();
            apply_output_policy_to_response(&mut response, PolicySource::Cassette)?;
            set_guarded_response_telemetry(span, &request, "cassette", &response);
            Ok(response)
        }
        crate::cassette::Decision::Miss(k) => Err(cassette_miss_error(&k)),
        crate::cassette::Decision::Record => {
            let mut resp = do_complete_inner(request.clone(), span)?;
            if let Err(error) = apply_output_policy_to_response(&mut resp, PolicySource::Request) {
                track_usage(&resp.usage)?;
                return Err(error);
            }
            set_guarded_response_telemetry(span, &request, "", &resp);
            let provider = LAST_SERVING_PROVIDER.with(|p| p.borrow().clone().unwrap_or_default());
            cassette_record(crate::cassette::TapeEntry::from_response(
                &key, &provider, &resp,
            ));
            Ok(resp)
        }
    }
}

pub(super) fn set_guarded_response_telemetry(
    span: &sema_otel::LlmSpan,
    request: &ChatRequest,
    provider_override: &str,
    response: &ChatResponse,
) {
    let provider = if provider_override.is_empty() {
        LAST_SERVING_PROVIDER.with(|provider| provider.borrow().clone().unwrap_or_default())
    } else {
        provider_override.to_string()
    };
    span.set_dispatch(&provider, &response.model);
    span.set_response(&response_facts(&provider, response));
    span.set_messages(
        &messages_json(&request.messages),
        &content_json("assistant", &response.content),
        request
            .system
            .as_deref()
            .map(|system| content_json("system", system))
            .as_deref(),
    );
}

/// The hard error raised on a `:replay`-mode cassette miss (no recorded interaction
/// for this request). Shared by the complete, stream, and embed seams.
pub(super) fn cassette_miss_error(key: &str) -> SemaError {
    SemaError::Llm(format!(
        "cassette miss in :replay mode (key {key}) — no recorded interaction for this \
         request; re-record the tape or use :auto mode"
    ))
}

pub(super) fn do_complete_inner(
    request: ChatRequest,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    let fallback_chain = FALLBACK_CHAIN.with(|c| c.borrow().clone());
    match fallback_chain {
        Some(chain) if !chain.is_empty() => {
            let mut last_error = None;
            for entry in &chain {
                match do_complete_with_provider(entry, request.clone(), span) {
                    Ok(Some(resp)) => return Ok(resp),
                    Ok(None) => continue,
                    Err(e) => {
                        eprintln!(
                            "Provider '{}' failed: {}, trying next...",
                            entry.provider, e
                        );
                        last_error = Some(e);
                    }
                }
            }
            let err = last_error.unwrap_or_else(|| SemaError::Llm("all providers failed".into()));
            span.record_error("provider_error", &err.to_string());
            Err(err)
        }
        _ => {
            let r = do_complete_uncached(request, span);
            if let Err(e) = &r {
                span.record_error("provider_error", &e.to_string());
            }
            r
        }
    }
}

thread_local! {
    /// Base delay for exponential backoff between network retries. Tests set this
    /// to 0 via [`set_retry_base_ms`] so retry behavior is asserted on attempt
    /// count without real sleeps.
    pub(super) static RETRY_BASE_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(500) };
    /// Max same-provider retries on transient errors (429 / 5xx / network).
    pub(super) static NETWORK_MAX_RETRIES: std::cell::Cell<u32> = const { std::cell::Cell::new(3) };
}

/// Test hook: set the retry backoff base (ms). 0 disables sleeping.
pub fn set_retry_base_ms(ms: u64) {
    RETRY_BASE_MS.with(|c| c.set(ms));
}

/// Test/config hook: set the max number of same-provider network retries.
pub fn set_network_max_retries(n: u32) {
    NETWORK_MAX_RETRIES.with(|c| c.set(n));
}

/// Whether an `LlmError` is worth retrying on the same provider, and the
/// server-suggested wait in ms. `Some(ms)`: retryable — `ms > 0` honors that wait
/// (429 `retry-after`), `ms == 0` means use computed backoff. `None`: not
/// retryable (4xx non-429, parse/config errors).
pub(super) fn retryable_wait(err: &crate::types::LlmError) -> Option<u64> {
    use crate::types::LlmError::*;
    match err {
        RateLimited { retry_after_ms } => Some(*retry_after_ms),
        // 5xx are transient server faults; network failures and timeouts surface
        // as Http(_). Both are safe to retry.
        Api { status, .. } if *status >= 500 => Some(0),
        Http(_) => Some(0),
        _ => None,
    }
}

/// Capped exponential backoff with full jitter. A positive server hint wins.
/// `base_ms` is the configured retry-backoff base, passed in explicitly (NOT read
/// from the `RETRY_BASE_MS` thread-local here) so the async wire stage — which
/// runs on pool worker threads with their own TLS copies — honors the base the
/// VM thread configured (incl. the `set_retry_base_ms(0)` test hook). The VM
/// thread captures the TL value and threads it down.
pub(super) fn retry_backoff_ms(attempt: u32, server_hint: u64, base_ms: u64) -> u64 {
    const CAP_MS: u64 = 30_000;
    if server_hint > 0 {
        return server_hint.min(CAP_MS);
    }
    let base = base_ms;
    if base == 0 {
        return 0;
    }
    let ceil = base.saturating_mul(1u64 << attempt.min(6)).min(CAP_MS);
    // Full jitter: a uniform-ish value in [0, ceil]. Sub-nanosecond entropy is
    // plenty here — jitter only affects sleep duration, never control flow.
    let entropy = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    entropy % (ceil + 1)
}

/// A single network-retry event, captured as DATA (not emitted as an otel span at
/// the point it happens). The synchronous completion path emits `retry_span`s
/// inline from these; the runtime path collects them on a worker thread (no otel TLS
/// there) and replays them as spans in the VM-thread finalizer. Capturing-as-data is
/// what lets both paths share one retry loop with zero telemetry drift.
#[derive(Debug, Clone)]
pub(super) struct RetryEvent {
    /// 1-based attempt number that triggered the retry (matches `retry_span`).
    attempt: u32,
    /// `llm_error_kind` of the error that triggered the retry.
    kind: &'static str,
    /// The error's display message.
    msg: String,
    /// The backoff actually applied before the retry, in ms.
    wait_ms: u64,
}

/// Run `provider.complete` with retry on transient errors (429 / 5xx / network),
/// using capped exponential backoff with jitter (429 honors `retry-after`),
/// COLLECTING each retry as a [`RetryEvent`] rather than emitting otel spans.
/// Synchronous-path loop (the VM thread; provider `block_on` already returned
/// before the backoff `thread::sleep`); the async wire stage uses the twin
/// [`complete_with_retry_collecting_async`]. Touches NO thread-locals.
pub(super) fn complete_with_retry_collecting(
    provider: &dyn LlmProvider,
    request: &ChatRequest,
    max_retries: u32,
    base_ms: u64,
) -> Result<(ChatResponse, Vec<RetryEvent>), crate::types::LlmError> {
    let mut attempt = 0u32;
    let mut events = Vec::new();
    loop {
        match provider.complete(request.clone()) {
            Ok(resp) => return Ok((resp, events)),
            Err(e) => match retryable_wait(&e) {
                Some(hint) if attempt < max_retries => {
                    let wait = retry_backoff_ms(attempt, hint, base_ms);
                    events.push(RetryEvent {
                        attempt: attempt + 1,
                        kind: llm_error_kind(&e),
                        msg: e.to_string(),
                        wait_ms: wait,
                    });
                    if wait > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(wait));
                    }
                    attempt += 1;
                }
                _ => return Err(e),
            },
        }
    }
}

/// Emit one `retry_span` child per collected [`RetryEvent`] under the active LLM
/// span. Called on the VM thread (synchronous path inline; runtime path in the finalizer)
/// where the otel context is live.
pub(super) fn emit_retry_spans(events: &[RetryEvent]) {
    for ev in events {
        let rspan = sema_otel::retry_span(ev.attempt);
        rspan.record_error(ev.kind, &ev.msg);
        rspan.set_wait_ms(ev.wait_ms);
    }
}

/// Run `provider.complete` with retry on transient errors (429 / 5xx / network),
/// using capped exponential backoff with jitter (429 honors `retry-after`).
/// Re-expressed on top of [`complete_with_retry_collecting`] so synchronous and runtime
/// paths share one retry loop: this variant emits the collected retries as otel
/// `retry_span` children inline (the sync path's behavior, unchanged).
pub(super) fn complete_with_retry(
    provider: &dyn LlmProvider,
    request: &ChatRequest,
    max_retries: u32,
) -> Result<ChatResponse, crate::types::LlmError> {
    // Sync path runs on the VM thread, so reading the TL base here is correct.
    let base_ms = RETRY_BASE_MS.with(|c| c.get());
    let (resp, events) = complete_with_retry_collecting(provider, request, max_retries, base_ms)?;
    emit_retry_spans(&events);
    Ok(resp)
}

/// One resolved fallback target for the offloadable wire stage: an `Arc` provider
/// clone (off the thread-local registry, cloned on the VM thread before offload),
/// the provider's registry name, and an optional per-entry model override.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub(super) struct ResolvedProvider {
    pub(super) provider: std::sync::Arc<dyn LlmProvider>,
    pub(super) name: String,
    pub(super) model: Option<String>,
}

/// Result of the offloadable completion wire stage: the response, the name of the
/// provider that served it (for `set_serving_provider` + pricing on the VM thread),
/// and the collected retry events (replayed as spans in the VM-thread finalizer).
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct CompleteOutcome {
    pub(super) resp: ChatResponse,
    pub(super) serving_provider: String,
    pub(super) serving_model: String,
    pub(super) retry_events: Vec<RetryEvent>,
}

/// Default per-job deadline (ms) for the sync-only provider blocking offload when the
/// request carries no explicit `:timeout`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const SYNC_ONLY_DEFAULT_TIMEOUT_MS: u64 = 300_000; // 5 minutes
/// Hard ceiling (ms) clamped over any caller-supplied `:timeout` for the sync-only
/// blocking offload, so a blocking `complete()` with no interrupt handle can never
/// occupy a worker unbounded.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const SYNC_ONLY_MAX_TIMEOUT_MS: u64 = 600_000; // 10 minutes

/// Resolve the sync-only blocking offload's per-job deadline BEFORE dispatch: a positive
/// caller `:timeout` (`request.timeout_ms`) is honored up to the hard ceiling; anything
/// missing or zero falls back to the default. The result is always finite, so an
/// unbounded blocking `complete()` is unrepresentable.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn sync_only_offload_deadline_ms(timeout_ms: Option<u64>) -> u64 {
    timeout_ms
        .filter(|&ms| ms > 0)
        .unwrap_or(SYNC_ONLY_DEFAULT_TIMEOUT_MS)
        .min(SYNC_ONLY_MAX_TIMEOUT_MS)
}

/// One completion attempt for the async wire stage. Providers with a native
/// async path (`complete_future`) are awaited in-place inside the spawned pool
/// future — aborting the task drops the in-flight request (TRUE cancellation,
/// connection torn down). Sync-only providers (the `complete_future` default —
/// e.g. the FakeProvider test double) fall back to an admission-controlled
/// blocking offload under a bounded per-job deadline
/// ([`sync_only_offload_deadline_ms`], resolved pre-dispatch). Cancellation on this
/// arm is explicitly a BEST-EFFORT QUARANTINE: a provider that exposes only a blocking
/// API has no interrupt handle, so on cancel or deadline the awaiting future is dropped
/// and the result discarded (never charged — `track_usage` runs only in the VM-thread
/// finalizer on resume) while the orphaned worker runs to completion on its own. There
/// is no fake abort.
#[cfg(not(target_arch = "wasm32"))]
async fn complete_once_async(
    provider: &std::sync::Arc<dyn LlmProvider>,
    request: &ChatRequest,
) -> Result<ChatResponse, crate::types::LlmError> {
    match provider.complete_future(request.clone()) {
        Some(fut) => fut.await,
        None => {
            let deadline =
                std::time::Duration::from_millis(sync_only_offload_deadline_ms(request.timeout_ms));
            let p = provider.clone();
            let req = request.clone();
            match tokio::time::timeout(
                deadline,
                sema_io::io_offload_blocking(move || p.complete(req)),
            )
            .await
            {
                Ok(result) => result,
                // Fail fast on the deadline (a non-retryable Config error): retrying a
                // purely-blocking API we cannot interrupt would only strand more workers.
                Err(_elapsed) => Err(crate::types::LlmError::Config(format!(
                    "sync-only provider blocking call exceeded its {} ms deadline \
                     (best-effort quarantine: the result is discarded and never charged; \
                     the orphaned worker runs to completion on its own)",
                    deadline.as_millis()
                ))),
            }
        }
    }
}

/// Async twin of [`complete_with_retry_collecting`] for the spawned wire stage:
/// same retry policy (429/5xx/network retryable with capped exponential
/// backoff + full jitter, 429 honors `retry-after`, 4xx-non-429 fail fast) and
/// the same collected [`RetryEvent`]s, but each attempt goes through
/// [`complete_once_async`] and the backoff is a `tokio::time::sleep` — so an
/// abort during either the attempt or the backoff drops the future instead of
/// stranding a blocking worker. Touches NO thread-locals.
#[cfg(not(target_arch = "wasm32"))]
async fn complete_with_retry_collecting_async(
    provider: &std::sync::Arc<dyn LlmProvider>,
    request: &ChatRequest,
    max_retries: u32,
    base_ms: u64,
) -> Result<(ChatResponse, Vec<RetryEvent>), crate::types::LlmError> {
    let mut attempt = 0u32;
    let mut events = Vec::new();
    loop {
        match complete_once_async(provider, request).await {
            Ok(resp) => return Ok((resp, events)),
            Err(e) => match retryable_wait(&e) {
                Some(hint) if attempt < max_retries => {
                    let wait = retry_backoff_ms(attempt, hint, base_ms);
                    events.push(RetryEvent {
                        attempt: attempt + 1,
                        kind: llm_error_kind(&e),
                        msg: e.to_string(),
                        wait_ms: wait,
                    });
                    if wait > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                    }
                    attempt += 1;
                }
                _ => return Err(e),
            },
        }
    }
}

pub(super) fn do_complete_with_provider(
    entry: &FallbackEntry,
    mut request: ChatRequest,
    span: &sema_otel::LlmSpan,
) -> Result<Option<ChatResponse>, SemaError> {
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = reg.get(&entry.provider).ok_or_else(|| {
            SemaError::Llm(format!("fallback provider '{}' not found", entry.provider))
        })?;
        // A per-provider chain override wins over any model pinned in the call body
        // (so the chain can target a different model per provider); otherwise fall
        // back to the provider's own default when nothing was pinned. Either way each
        // provider receives a model id valid for itself.
        if let Some(model) = &entry.model {
            request.model = model.clone();
        } else if request.model.is_empty() {
            request.model = provider.default_model().to_string();
        }
        if !model_target_allowed(&entry.provider, &request.model, PolicySource::Request, true)? {
            return Ok(None);
        }
        let max_retries = NETWORK_MAX_RETRIES.with(|c| c.get());
        let resp = complete_with_retry(&*provider, &request, max_retries)
            .map_err(|e| SemaError::Llm(e.to_string()))?;
        set_serving_provider(&entry.provider);
        // Provider + model + response are all in scope here, before track_usage
        // consumes the serving-provider stamp.
        span.set_dispatch(&entry.provider, &request.model);
        Ok(Some(resp))
    })
}

pub(super) fn do_complete_uncached(
    mut request: ChatRequest,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    enforce_rate_limit();
    let max_retries = NETWORK_MAX_RETRIES.with(|c| c.get());
    with_provider(|p| {
        if request.model.is_empty() {
            request.model = p.default_model().to_string();
        }
        model_target_allowed(p.name(), &request.model, PolicySource::Request, false)?;
        let resp = complete_with_retry(p, &request, max_retries)
            .map_err(|e| SemaError::Llm(e.to_string()))?;
        set_serving_provider(p.name());
        // Capture provider/model/response before track_usage consumes the stamp.
        span.set_dispatch(p.name(), &request.model);
        Ok(resp)
    })
}

pub(super) fn enforce_rate_limit() {
    let rps = RATE_LIMIT_RPS.with(|r| r.get());
    if let Some(rps) = rps {
        let min_interval_ms = (1000.0 / rps) as u64;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let last = rate_limit_last_value();
        // saturating_sub: a backward wall-clock adjustment makes `now < last`,
        // which would panic (debug) or wrap to a huge value (release) on plain
        // subtraction. Treat that as "no wait needed". This sleep runs on the
        // synchronous caller thread (the provider's own block_on has already
        // returned), so it does not stall a shared tokio runtime worker.
        let elapsed = now.saturating_sub(last);
        if last > 0 && elapsed < min_interval_ms {
            let sleep_ms = min_interval_ms - elapsed;
            std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
        }
        let actual_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        set_rate_limit_last_value(actual_now);
    }
}

/// Non-blocking counterpart to `enforce_rate_limit`, used by completion and
/// streaming offloads. Runs synchronously on the VM thread before dispatch and
/// never sleeps
/// itself. It returns how many milliseconds THIS call's send must be delayed
/// (0 if the gate is clear), and the caller is responsible for spending that
/// delay somewhere that isn't the VM thread (a `tokio::time::sleep` inside the
/// offloaded future, or a `std::thread::sleep` on a pool worker) so sibling
/// tasks keep running while the pacing gap elapses.
///
/// Reserve-then-go: unlike `enforce_rate_limit` (which stamps `RATE_LIMIT_LAST`
/// to the actual wall-clock time AFTER it wakes from its own blocking sleep),
/// this stamps it to the RESERVED slot — which may be in the future — before
/// returning. The scheduler is single-threaded, so a burst of async calls each
/// run this function to completion, one at a time, in dispatch order; a later
/// call in the same burst sees the earlier one's reservation already advanced
/// and queues one interval further out. Without reserving up front, every call
/// in the burst would read the same stale `RATE_LIMIT_LAST` (none of them have
/// sent yet — their waits run on background workers) and compute the same
/// wait, so they'd all fire together the instant it elapses instead of staying
/// paced.
pub(super) fn reserve_rate_limit_wait_ms() -> u64 {
    let rps = RATE_LIMIT_RPS.with(|r| r.get());
    let Some(rps) = rps else {
        return 0;
    };
    let min_interval_ms = (1000.0 / rps) as u64;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last = rate_limit_last_value();
    // A `last` slot more than this far ahead of `now` cannot be a legitimate
    // reservation queue — the wall clock jumped backward since it was
    // stamped. Discard it, exactly like `enforce_rate_limit`'s
    // `saturating_sub` guard for the same condition (see
    // `enforce_rate_limit_survives_backward_clock`), so a corrupted `last`
    // cannot wedge a call behind a real multi-minute wait. Scaled to
    // `min_interval_ms` (floored at 60s) rather than a bare constant: a very
    // low configured rps can legitimately reserve slots far ahead of `now`
    // after just a few concurrent dispatches (e.g. 0.05 rps ⇒ 20s apart), and
    // a fixed cap too close to one interval would misclassify that as clock
    // skew and silently under-pace it.
    const MIN_TRUSTED_RESERVATION_AHEAD_MS: u64 = 60_000;
    let max_trusted_ahead_ms = min_interval_ms
        .saturating_mul(4)
        .max(MIN_TRUSTED_RESERVATION_AHEAD_MS);
    let last = if last > now.saturating_add(max_trusted_ahead_ms) {
        0
    } else {
        last
    };
    // No prior dispatch/reservation (or the stale value just discarded above):
    // this call's slot is now — no wait, nothing to reserve ahead of `now`.
    let slot = if last == 0 {
        now
    } else {
        now.max(last.saturating_add(min_interval_ms))
    };
    set_rate_limit_last_value(slot);
    slot.saturating_sub(now)
}

pub(super) fn register(env: &Env) {
    register_scope_fn_ctx(env, "llm/with-fallback", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/with-fallback", "2", args.len()));
        }
        let providers = args.seq_at(0, "llm/with-fallback")?;
        let body_fn = &args[1];
        if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", body_fn.type_name()));
        }
        let chain: Vec<FallbackEntry> = providers
            .iter()
            .map(parse_fallback_entry)
            .collect::<Result<_, _>>()?;
        let prev = FALLBACK_CHAIN.with(|c| c.borrow().clone());
        FALLBACK_CHAIN.with(|c| *c.borrow_mut() = Some(chain));
        Ok((
            body_fn.clone(),
            Box::new(move || {
                FALLBACK_CHAIN.with(|c| *c.borrow_mut() = prev);
            }),
        ))
    });

    register_scope_fn_ctx(env, "llm/with-rate-limit", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/with-rate-limit", "2", args.len()));
        }
        let rps = args[0]
            .as_float()
            .or_else(|| args[0].as_int().map(|i| i as f64))
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let body_fn = &args[1];
        if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", body_fn.type_name()));
        }
        let prev = RATE_LIMIT_RPS.with(|r| r.get());
        let prev_last = RATE_LIMIT_LAST.with(|last| last.borrow().clone());
        RATE_LIMIT_RPS.with(|r| r.set(Some(rps)));
        RATE_LIMIT_LAST.with(|last| *last.borrow_mut() = Some(Rc::new(Cell::new(0))));
        Ok((
            body_fn.clone(),
            Box::new(move || {
                RATE_LIMIT_RPS.with(|r| r.set(prev));
                RATE_LIMIT_LAST.with(|last| *last.borrow_mut() = prev_last);
            }),
        ))
    });

    // --- Convenience wrappers ---

    // (llm/io-sleep-once id [ms]) — external-wait test leaf (NOT for production use).
    //
    // Mimics `llm/chat-once` but does a timer instead of an HTTP call: spawns a
    // `tokio::time::sleep` on the I/O pool and suspends on an External wait, so
    // the runtime can drive sibling tasks. This proves real overlap before any
    // agent-loop work. Resolves to `id`.
    #[cfg(not(target_arch = "wasm32"))]
    register_runtime_fn_ctx(env, "llm/io-sleep-once", |_ctx, args| {
        use sema_core::runtime::NativeOutcome;
        use std::sync::atomic::Ordering;

        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/io-sleep-once", "1-2", args.len()));
        }
        let id = args[0].as_int().unwrap_or(0);
        let ms = args.get(1).and_then(|v| v.as_int()).unwrap_or(1000).max(0) as u64;

        // A runtime root or spawned task suspends on an External wait backed by an
        // async-tier timer. The in-flight gauge is bumped on the VM thread so a
        // test can prove simultaneity before the future's first poll; the future
        // decrements it.
        if in_runtime_offload_task() {
            use sema_core::runtime::{
                CompletionKind, InterruptibleResource, NativeSuspend, PreparedExternalOperation,
                SendPayload, WaitKind,
            };
            let prev = IO_INFLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
            IO_PEAK.fetch_max(prev, Ordering::SeqCst);
            let kind = CompletionKind::try_from_raw(IO_SLEEP_COMPLETION_KIND)
                .expect("io-sleep completion kind is nonzero");
            let resource =
                InterruptibleResource::new("llm/io-sleep-once", Box::new(CompleteNoopCancelHook));
            let prepared = PreparedExternalOperation::interruptible_async(
                kind,
                Box::new(IoSleepDecoder),
                resource,
                move || async move {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    let _ = IO_INFLIGHT
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some((v - 1).max(0)));
                    Ok(Box::new(id) as SendPayload)
                },
            );
            return Ok(NativeOutcome::Suspend(NativeSuspend {
                wait: WaitKind::External(Box::new(prepared)),
                continuation: Box::new(OffloadValueContinuation {
                    op: "llm/io-sleep-once",
                }),
            }));
        }

        // Host/plain-callback fallback: sleep synchronously and return the id.
        // The in-flight gauge is bumped/decremented
        // around the blocking sleep so the concurrency test invariants still hold.
        let prev = IO_INFLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        IO_PEAK.fetch_max(prev, Ordering::SeqCst);
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::sleep(std::time::Duration::from_millis(ms));
        let _ =
            IO_INFLIGHT.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some((v - 1).max(0)));
        Ok(NativeOutcome::Return(Value::int(id)))
    });
}
