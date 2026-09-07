use super::*;

/// Completion-kind tag for an `llm/batch` round offloaded through the unified
/// runtime's External-wait machinery.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const BATCH_COMPLETION_KIND: u64 = 0x6261_7463; // "batc"

/// Cooperative sequencer for a Sema-defined provider's default `batch_complete`
/// behavior. Every request callback runs in input order, including callbacks after
/// an earlier failure; only after the sequence settles are results folded in order.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct SemaBatchDriver {
    provider: String,
    default_model: String,
    requests: Vec<ChatRequest>,
    next_request: usize,
    active_model: Option<String>,
    responses: Vec<Result<ChatResponse, LlmError>>,
    usage_accum_slot: Option<Rc<RefCell<LeafUsage>>>,
    budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for SemaBatchDriver {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl SemaBatchDriver {
    fn advance(mut self: Box<Self>) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeCall, NativeOutcome};

        while let Some(request) = self.requests.get(self.next_request) {
            let mut request = request.clone();
            self.next_request += 1;
            if request.model.is_empty() {
                request.model = self.default_model.clone();
            }
            let callback = match lisp_provider_complete_callback(&self.provider) {
                Ok(callback) => callback,
                Err(error) => {
                    self.responses.push(Err(error));
                    continue;
                }
            };
            self.active_model = Some(request.model.clone());
            return Ok(NativeOutcome::Call(NativeCall {
                callable: callback,
                args: vec![chat_request_to_value(&request)],
                continuation: self,
            }));
        }

        let Self {
            responses,
            usage_accum_slot,
            budget_slot,
            ..
        } = *self;
        finalize_batch_responses(responses, usage_accum_slot, budget_slot)
            .map(NativeOutcome::Return)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::NativeContinuation for SemaBatchDriver {
    fn resume(
        mut self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;

        match input {
            ResumeInput::Returned(value) => {
                let model = self
                    .active_model
                    .take()
                    .expect("Sema batch callback has an active model");
                self.responses
                    .push(parse_lisp_provider_response(&value, &model));
                self.advance()
            }
            ResumeInput::Failed(error) => {
                self.active_model.take();
                self.responses.push(Err(LlmError::Api {
                    status: 0,
                    message: error.to_string(),
                }));
                self.advance()
            }
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "llm/batch callback was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "llm/batch callback received an unexpected runtime response",
            )),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn finalize_batch_responses(
    responses: Vec<Result<ChatResponse, LlmError>>,
    usage_accum_slot: Option<Rc<RefCell<LeafUsage>>>,
    budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
) -> Result<Value, SemaError> {
    let mut results = Vec::with_capacity(responses.len());
    for resp_result in responses {
        let mut resp = resp_result.map_err(|error| SemaError::Llm(error.to_string()))?;
        let policy_result = apply_output_policy_to_response(&mut resp, PolicySource::Request);
        // Priced with an empty provider, matching the sync path (which never
        // stamps a serving provider for `llm/batch`).
        if let Some(slot) = &usage_accum_slot {
            let cost = pricing::calculate_cost_for("", &resp.usage);
            accumulate_into(slot, &resp.usage, cost);
        }
        let prev_budget = ACTIVE_BUDGET
            .with(|active| std::mem::replace(&mut *active.borrow_mut(), budget_slot.clone()));
        let track_result = USAGE_ACCUM_SUPPRESS.with(|suppress| {
            suppress.set(true);
            let result = track_usage(&resp.usage);
            suppress.set(false);
            result
        });
        ACTIVE_BUDGET.with(|active| *active.borrow_mut() = prev_budget);
        track_result?;
        policy_result?;
        results.push(Value::string(&resp.content));
    }
    Ok(Value::list(results))
}

/// Decoder for an offloaded `llm/batch`: folds each response into the
/// dispatch-time leaf/budget frames and shapes the result list on the VM thread.
/// Holds no live `Value`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct BatchDecoder {
    usage_accum_slot: Option<Rc<RefCell<LeafUsage>>>,
    budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for BatchDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::CompletionDecoder for BatchDecoder {
    fn decode(
        self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> sema_core::runtime::DecodedCompletion {
        let payload = match result {
            Ok(payload) => payload,
            Err(failure) => return Err(SemaError::eval(format!("batch: {}", failure.message()))),
        };
        let responses = match sema_core::runtime::downcast_send_payload::<
            Vec<Result<ChatResponse, LlmError>>,
        >(payload, "batch")
        {
            Ok(responses) => responses,
            Err(failure) => return Err(SemaError::eval(format!("batch: {}", failure.message()))),
        };
        finalize_batch_responses(responses, self.usage_accum_slot, self.budget_slot)
    }
}

pub(super) fn register(env: &Env) {
    // The public `llm/pmap` is a prelude dispatcher. Runtime tasks compose
    // structural `map` + `llm/batch`; host callback entry routes here so the
    // mapper receives the caller's explicit EvalContext rather than STDLIB_CTX.
    // The guard makes this compatibility path impossible to use as a blocking
    // escape hatch from an active runtime quantum.
    register_fn_ctx(env, "__llm-pmap-blocking", |ctx, args| {
        if sema_core::in_runtime_quantum() {
            return Err(SemaError::eval(
                "__llm-pmap-blocking cannot run inside the cooperative runtime",
            )
            .with_hint("call llm/pmap so its mapper can suspend cooperatively"));
        }
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/pmap", "2-3", args.len()));
        }
        let func = &args[0];
        let items = args[1]
            .as_seq()
            .map(|items| items.to_vec())
            .ok_or_else(|| SemaError::type_error("list or vector", args[1].type_name()))?;

        let mut model = String::new();
        let mut max_tokens = None;
        let mut temperature = None;
        let mut system = None;
        if let Some(opts) = args.get(2).and_then(Value::as_map_rc) {
            model = opts.opt_str("model").unwrap_or_default();
            max_tokens = opts.opt_int("max-tokens").map(|n| n as u32);
            temperature = opts.opt_f64("temperature");
            system = opts.opt_str("system");
        }

        let prompts = items
            .iter()
            .map(|item| {
                let result = sema_core::call_callback(ctx, func, std::slice::from_ref(item))?;
                Ok(result
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| result.to_string()))
            })
            .collect::<Result<Vec<_>, SemaError>>()?;
        let requests = prompts
            .into_iter()
            .map(|prompt_text| {
                let mut request =
                    ChatRequest::new(model.clone(), vec![ChatMessage::new("user", prompt_text)]);
                request.max_tokens = max_tokens.or(Some(4096));
                request.temperature = temperature;
                request.system = system.clone();
                request
            })
            .collect::<Vec<_>>();

        let responses = with_provider(|provider| {
            let requests = resolve_batch_models(provider, requests)?;
            Ok(provider.batch_complete(requests))
        })?;
        responses
            .into_iter()
            .map(|response| {
                let mut response = response.map_err(|error| SemaError::Llm(error.to_string()))?;
                if let Err(error) =
                    apply_output_policy_to_response(&mut response, PolicySource::Request)
                {
                    track_usage(&response.usage)?;
                    return Err(error);
                }
                track_usage(&response.usage)?;
                Ok(Value::string(&response.content))
            })
            .collect::<Result<Vec<_>, SemaError>>()
            .map(Value::list)
    });

    // These non-blocking helpers preserve the public native's exact validation
    // diagnostics while the prelude owns cooperative control flow.
    register_fn(env, "__llm-pmap-arity-error", |args| {
        Err(SemaError::arity("llm/pmap", "2-3", args.len()))
    });
    register_fn(env, "__llm-pmap-validate-items", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity(
                "__llm-pmap-validate-items",
                "1",
                args.len(),
            ));
        }
        args[0]
            .as_seq()
            .ok_or_else(|| SemaError::type_error("list or vector", args[0].type_name()))?;
        Ok(args[0].clone())
    });

    // (llm/batch ["prompt1" "prompt2" "prompt3"] {:max-tokens 100})
    register_runtime_fn_ctx(env, "llm/batch", |_ctx, args| {
        #[allow(unused_imports)]
        use sema_core::runtime::NativeOutcome;
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/batch", "1-2", args.len()));
        }
        let prompts = args.seq_at(0, "llm/batch")?.to_vec();

        let mut model = String::new();
        let mut max_tokens = None;
        let mut temperature = None;
        let mut system = None;

        if let Some(opts_val) = args.get(1) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = opts.opt_str("model").unwrap_or_default();
                max_tokens = opts.opt_int("max-tokens").map(|n| n as u32);
                temperature = opts.opt_f64("temperature");
                system = opts.opt_str("system");
            }
        }

        let requests: Vec<ChatRequest> = prompts
            .iter()
            .map(|prompt_val| {
                let prompt_text = prompt_val
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| prompt_val.to_string());
                let messages = vec![ChatMessage::new("user", prompt_text)];
                let mut req = ChatRequest::new(model.clone(), messages);
                req.max_tokens = max_tokens.or(Some(4096));
                req.temperature = temperature;
                req.system = system.clone();
                req
            })
            .collect();

        // ── Runtime path ────────────────────────────────────────────────────
        //
        // Sema-defined providers run one structural callback per request, in order.
        // Native providers retain their own `batch_complete` concurrency and run the
        // whole batch as one admission-controlled blocking-tier unit.
        #[cfg(not(target_arch = "wasm32"))]
        if in_runtime_offload_context() {
            let provider = PROVIDER_REGISTRY.with(|reg| reg.borrow().default_provider());
            let Some(provider) = provider else {
                return Err(SemaError::Llm(
                    "no LLM provider configured. Use (llm/configure :anthropic {:api-key ...}) \
                     first"
                        .to_string(),
                ));
            };
            let resolved_requests = resolve_batch_models(&*provider, requests.iter().cloned())?;

            if provider.runs_on_vm_thread() {
                return Box::new(SemaBatchDriver {
                    provider: provider.name().to_string(),
                    default_model: provider.default_model().to_string(),
                    requests: resolved_requests,
                    next_request: 0,
                    active_model: None,
                    responses: Vec::new(),
                    usage_accum_slot: current_usage_accum(),
                    budget_slot: active_budget(),
                })
                .advance();
            }

            // Capture the dispatch-time budget + leaf-usage frames (ASYNC-1), so the
            // decoder charges the frames active now, not whatever scope is installed
            // when the future lands. Mirrors the completion and embedding paths.
            let usage_accum_slot = current_usage_accum();
            let budget_slot = active_budget();

            // A unified-runtime spawned task SUSPENDS on an External wait (the whole
            // batch runs as ONE unit on the executor's blocking tier, since
            // `batch_complete` drives its own internal `join_all`); the decoder folds
            // usage and shapes the list on the VM thread.
            if in_runtime_offload_task() {
                use sema_core::runtime::{
                    CompletionKind, InterruptibleResource, NativeSuspend,
                    PreparedExternalOperation, SendPayload, WaitKind,
                };
                let p_job = provider.clone();
                let decoder = Box::new(BatchDecoder {
                    usage_accum_slot: usage_accum_slot.clone(),
                    budget_slot: budget_slot.clone(),
                });
                let kind = CompletionKind::try_from_raw(BATCH_COMPLETION_KIND)
                    .expect("batch completion kind is nonzero");
                let resource =
                    InterruptibleResource::new("llm/batch", Box::new(CompleteNoopCancelHook));
                let prepared = PreparedExternalOperation::interruptible_blocking(
                    kind,
                    decoder,
                    resource,
                    move || Ok(Box::new(p_job.batch_complete(resolved_requests)) as SendPayload),
                );
                return Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::External(Box::new(prepared)),
                    continuation: Box::new(OffloadValueContinuation { op: "llm/batch" }),
                }));
            }
        }

        let responses = with_provider(|p| {
            let reqs = resolve_batch_models(p, requests)?;
            Ok(p.batch_complete(reqs))
        })?;

        let mut results = Vec::with_capacity(responses.len());
        for resp_result in responses {
            let mut resp = resp_result.map_err(|e| SemaError::Llm(e.to_string()))?;
            if let Err(error) = apply_output_policy_to_response(&mut resp, PolicySource::Request) {
                track_usage(&resp.usage)?;
                return Err(error);
            }
            track_usage(&resp.usage)?;
            results.push(Value::string(&resp.content));
        }
        Ok(NativeOutcome::Return(Value::list(results)))
    });
}
