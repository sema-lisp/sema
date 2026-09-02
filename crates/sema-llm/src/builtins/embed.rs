use super::*;

pub(super) fn require_matching_bytevectors<'a>(
    name: &str,
    args: &'a [Value],
) -> Result<(&'a [u8], &'a [u8]), SemaError> {
    if args.len() != 2 {
        return Err(SemaError::arity(name, "2", args.len()));
    }
    let a = args.bytes_at(0, name)?;
    let b = args.bytes_at(1, name)?;
    if a.len() != b.len() {
        return Err(SemaError::eval(format!(
            "{name}: length mismatch ({} vs {})",
            a.len() / 8,
            b.len() / 8
        )));
    }
    if a.is_empty() || a.len() % 8 != 0 {
        return Err(SemaError::eval(format!(
            "{name}: invalid bytevector length {}",
            a.len()
        )));
    }
    Ok((a, b))
}

pub(super) fn extract_float_vec(val: &Value) -> Result<Vec<f64>, SemaError> {
    let items = val
        .as_seq()
        .ok_or_else(|| SemaError::type_error("list of numbers", val.type_name()))?;
    items
        .iter()
        .map(|v| {
            v.as_float()
                .ok_or_else(|| SemaError::type_error("number", v.type_name()))
        })
        .collect()
}

/// Completion-kind tag for an `llm/embed` round offloaded through the unified
/// runtime's External-wait machinery.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const EMBED_COMPLETION_KIND: u64 = 0x656d_6264; // "embd"

/// Decoder for an offloaded `llm/embed`: finalizes the detached span, records the
/// cassette, decodes the embedding, and accounts usage against the DISPATCH-TIME
/// leaf/budget frames on the VM thread when the wire future lands. Holds no live
/// `Value` (the span, model
/// strings, and captured slots are not `Value`s), so it emits no GC edges.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct EmbedDecoder {
    span: Option<sema_otel::LlmSpan>,
    provider_name: String,
    req_model: String,
    recording: bool,
    key: String,
    cassette_scope: Option<CassetteScope>,
    single: bool,
    usage_accum_slot: Option<Rc<RefCell<LeafUsage>>>,
    budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for EmbedDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::CompletionDecoder for EmbedDecoder {
    fn decode(
        mut self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> sema_core::runtime::DecodedCompletion {
        let payload = match result {
            Ok(payload) => payload,
            Err(failure) => {
                if let Some(span) = self.span.take() {
                    span.record_error("io", failure.message());
                }
                return Err(SemaError::eval(format!("embed: {}", failure.message())));
            }
        };
        let wire = match sema_core::runtime::downcast_send_payload::<Result<EmbedResponse, LlmError>>(
            payload, "embed",
        ) {
            Ok(wire) => wire,
            Err(failure) => {
                if let Some(span) = self.span.take() {
                    span.record_error("io", failure.message());
                }
                return Err(SemaError::eval(format!("embed: {}", failure.message())));
            }
        };
        match wire {
            Ok(resp) => {
                if let Some(span) = self.span.take() {
                    span.set_dispatch(&self.provider_name, &self.req_model);
                    span.set_response(&sema_otel::ResponseFacts {
                        input_tokens: resp.usage.prompt_tokens,
                        output_tokens: 0,
                        response_model: resp.model.clone(),
                        cost_usd: pricing::calculate_cost_for(&self.provider_name, &resp.usage),
                        ..Default::default()
                    });
                }
                if self.recording {
                    cassette_scope_record(
                        &self.cassette_scope,
                        crate::cassette::TapeEntry::from_embed(
                            &self.key,
                            &self.provider_name,
                            &resp.model,
                            &resp.embeddings,
                            resp.usage.prompt_tokens,
                        ),
                    );
                }
                if let Some(slot) = &self.usage_accum_slot {
                    let cost = pricing::calculate_cost_for(&self.provider_name, &resp.usage);
                    accumulate_into(slot, &resp.usage, cost);
                }
                let value = embed_value_from_response(&resp, self.single);
                let prev_budget = ACTIVE_BUDGET
                    .with(|b| std::mem::replace(&mut *b.borrow_mut(), self.budget_slot.clone()));
                let track_result = USAGE_ACCUM_SUPPRESS.with(|s| {
                    s.set(true);
                    let r = track_usage(&resp.usage);
                    s.set(false);
                    r
                });
                ACTIVE_BUDGET.with(|b| *b.borrow_mut() = prev_budget);
                track_result?;
                Ok(value)
            }
            Err(e) => {
                if let Some(span) = self.span.take() {
                    span.record_error(llm_error_kind(&e), &e.to_string());
                }
                Err(SemaError::Llm(e.to_string()))
            }
        }
    }
}

/// Completion-kind tag for an `llm/rerank` round offloaded through the unified
/// runtime's External-wait machinery.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const RERANK_COMPLETION_KIND: u64 = 0x7272_6e6b; // "rrnk"

/// Decoder for an offloaded `llm/rerank`: finalizes the detached reranker span and
/// builds the reordered result list on the VM thread when the wire future lands.
/// `documents` is plain `String` data (not `Value`s), so it emits no GC edges.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct RerankDecoder {
    span: Option<sema_otel::RerankerSpan>,
    documents: Vec<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for RerankDecoder {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::CompletionDecoder for RerankDecoder {
    fn decode(
        mut self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        result: Result<sema_core::runtime::SendPayload, sema_core::runtime::ExternalFailure>,
    ) -> sema_core::runtime::DecodedCompletion {
        let payload = match result {
            Ok(payload) => payload,
            Err(failure) => {
                if let Some(span) = self.span.take() {
                    span.record_error("io", failure.message());
                }
                return Err(SemaError::eval(format!("rerank: {}", failure.message())));
            }
        };
        let wire = match sema_core::runtime::downcast_send_payload::<Result<RerankResponse, LlmError>>(
            payload, "rerank",
        ) {
            Ok(wire) => wire,
            Err(failure) => {
                if let Some(span) = self.span.take() {
                    span.record_error("io", failure.message());
                }
                return Err(SemaError::eval(format!("rerank: {}", failure.message())));
            }
        };
        match wire {
            Ok(resp) => {
                if let Some(span) = self.span.take() {
                    let out_docs: Vec<(String, f64)> = resp
                        .results
                        .iter()
                        .filter_map(|r| self.documents.get(r.index).map(|d| (d.clone(), r.score)))
                        .collect();
                    span.set_output(&out_docs);
                }
                Ok(rerank_value_from_response(&resp, &self.documents))
            }
            Err(e) => {
                if let Some(span) = self.span.take() {
                    span.record_error(llm_error_kind(&e), &e.to_string());
                }
                Err(SemaError::Llm(e.to_string()))
            }
        }
    }
}

/// Cassette key for an embeddings request (model + the input texts).
pub(super) fn compute_embed_key(request: &EmbedRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"embed");
    if let Some(ref m) = request.model {
        hasher.update(m.as_bytes());
    }
    for t in &request.texts {
        hasher.update(t.as_bytes());
        hasher.update(b"\0");
    }
    let policy_fingerprint = effective_policy_fingerprint();
    if !policy_fingerprint.is_empty() {
        hasher.update(b"\0policy\0");
        hasher.update(policy_fingerprint.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Encode an `EmbedResponse` for both synchronous and async calls. A single
/// vector becomes a bytevector; multiple vectors become a list of bytevectors.
pub(super) fn embed_value_from_response(resp: &EmbedResponse, single: bool) -> Value {
    if single {
        let embedding = resp.embeddings.first().cloned().unwrap_or_default();
        let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        Value::bytevector(bytes)
    } else {
        Value::list(
            resp.embeddings
                .iter()
                .map(|emb| {
                    let bytes: Vec<u8> = emb.iter().flat_map(|f| f.to_le_bytes()).collect();
                    Value::bytevector(bytes)
                })
                .collect(),
        )
    }
}

/// Encode a `RerankResponse` for both synchronous and async calls. Results are
/// ordered by relevance and contain `:index`, `:score`, and `:document`.
pub(super) fn rerank_value_from_response(resp: &RerankResponse, documents: &[String]) -> Value {
    Value::list(
        resp.results
            .iter()
            .map(|r| {
                let mut m = BTreeMap::new();
                m.insert(Value::keyword("index"), Value::int(r.index as i64));
                m.insert(Value::keyword("score"), Value::float(r.score));
                m.insert(
                    Value::keyword("document"),
                    Value::string(documents.get(r.index).map(|s| s.as_str()).unwrap_or("")),
                );
                Value::map(m)
            })
            .collect(),
    )
}

pub(super) fn register(env: &Env, unrestricted: bool) {
    // (llm/configure-embeddings :openai {:api-key "..." :base-url "..." :model "..."})
    // (llm/configure-embeddings :jina {:api-key "..."})
    // (llm/configure-embeddings :voyage {:api-key "..."})
    // (llm/configure-embeddings :cohere {:api-key "..."})
    register_fn(env, "llm/configure-embeddings", move |args| {
        if args.len() != 2 {
            return Err(SemaError::arity(
                "llm/configure-embeddings",
                "2",
                args.len(),
            ));
        }
        let provider_name = args.keyword_at(0, "llm/configure-embeddings")?;
        let opts_rc = args.map_at(1, "llm/configure-embeddings")?;
        let opts = opts_rc.as_ref().clone();

        guard_provider_url(unrestricted, &opts)?;

        let api_key = opts.opt_str("api-key");

        PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            match provider_name.as_str() {
                "jina" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts
                        .opt_str("default-model")
                        .unwrap_or_else(|| "jina-embeddings-v3".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "jina".to_string(),
                        api_key,
                        "https://api.jina.ai/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("jina");
                }
                "voyage" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts
                        .opt_str("default-model")
                        .unwrap_or_else(|| "voyage-3".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "voyage".to_string(),
                        api_key,
                        "https://api.voyageai.com/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("voyage");
                }
                "cohere" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = opts.opt_str("default-model");
                    let provider = CohereEmbeddingProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("cohere");
                }
                _ => {
                    // Default: OpenAI-compatible
                    let api_key = api_key.unwrap_or_default();
                    let base_url = opts.opt_str("base-url");
                    let model = opts
                        .opt_str("default-model")
                        .or_else(|| opts.opt_str("model"));
                    let provider = OpenAiProvider::new(api_key, base_url, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    let name = provider.name().to_string();
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider(&name);
                }
            }
            Ok(Value::nil())
        })
    });

    // `llm/embed` — a single first-class native function that branches internally
    // on whether it runs in a runtime quantum:
    //
    //   (llm/embed "text" {:model "..."})        ; → bytevector
    //   (llm/embed ["text1" "text2"] {:model …}) ; → list of bytevectors
    //
    // Outside the runtime it runs the synchronous embed path inline
    // (open span, cassette, provider.embed, set_response, track_usage, decode).
    // Root-main and spawned tasks offload `provider.embed` and suspend on an
    // External wait so siblings overlap. The VM-thread decoder finalizes the
    // detached span, records the cassette, runs `track_usage`, and builds the same
    // `Value` as the synchronous path.
    //
    // Keeping it a native (not a macro) means `(procedure? llm/embed)` is #t and
    // it is usable as a value: `(map llm/embed …)`, `(async/pool-map llm/embed …)`.
    register_runtime_fn_ctx(env, "llm/embed", |_ctx, args| {
        #[allow(unused_imports)]
        use sema_core::runtime::NativeOutcome;
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/embed", "1-2", args.len()));
        }

        let (texts, single) = if let Some(s) = args[0].as_str() {
            (vec![s.to_string()], true)
        } else if let Some(l) = args[0].as_seq() {
            let texts: Vec<String> = l
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| v.to_string())
                })
                .collect();
            (texts, false)
        } else {
            return Err(SemaError::type_error("string or list", args[0].type_name()));
        };

        let model = if let Some(opts_val) = args.get(1) {
            if let Some(opts) = opts_val.as_map_rc() {
                opts.opt_str("model")
            } else {
                None
            }
        } else {
            None
        };

        let mut request = EmbedRequest { texts, model };
        apply_input_policy_to_texts(&mut request.texts, "embedding.input")?;
        let req_model = request.model.clone().unwrap_or_default();
        let cassette_key = compute_embed_key(&request);

        // ── Runtime path: offload and suspend (native targets only) ─────────
        //
        // The concurrent embed path is native-only (no shared tokio runtime on
        // wasm), so wasm always falls through to the synchronous path below.
        #[cfg(not(target_arch = "wasm32"))]
        if in_runtime_offload_context() {
            // DETACHED embeddings span: parent captured now, finalized by the
            // decoder after the wait (where the active-span stack may hold a
            // sibling task's span, so the span must not pop the stack on drop).
            let span = sema_otel::llm_span_detached("embeddings");
            span.set_embedding_input(&request.texts);

            // Cassette decision — SYNCHRONOUSLY, pre-spawn, on the VM thread.
            let cassette_scope = current_cassette_scope();
            let decision = cassette_scope
                .as_ref()
                .map(|scope| scope.borrow().decide(&cassette_key));
            match decision {
                Some(crate::cassette::Decision::Replay(entry)) => {
                    enforce_stored_model_policy(
                        &entry.provider,
                        &entry.model,
                        PolicySource::Cassette,
                    )?;
                    // Replay made no provider call: finalize the span inline,
                    // account, and return without suspending.
                    let resp = EmbedResponse {
                        embeddings: entry.embeddings,
                        model: entry.model.clone(),
                        usage: Usage {
                            prompt_tokens: entry.prompt_tokens,
                            model: entry.model,
                            ..Default::default()
                        },
                    };
                    span.set_dispatch("cassette", &req_model);
                    span.set_response(&sema_otel::ResponseFacts {
                        input_tokens: resp.usage.prompt_tokens,
                        output_tokens: 0,
                        response_model: resp.model.clone(),
                        ..Default::default()
                    });
                    drop(span);
                    track_usage(&resp.usage)?;
                    return Ok(NativeOutcome::Return(embed_value_from_response(
                        &resp, single,
                    )));
                }
                Some(crate::cassette::Decision::Miss(k)) => return Err(cassette_miss_error(&k)),
                _ => {}
            }
            let recording = matches!(decision, Some(crate::cassette::Decision::Record));

            let provider = PROVIDER_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                reg.embedding_provider().or_else(|| reg.default_provider())
            });
            let Some(provider) = provider else {
                return Err(SemaError::Llm(
                    "no embedding provider configured. Use (llm/configure-embeddings ...) first"
                        .to_string(),
                ));
            };
            let model = request
                .model
                .as_deref()
                .unwrap_or_else(|| provider.default_model());
            model_target_allowed(provider.name(), model, PolicySource::Request, false)?;

            // The provider name + canonical price are needed on the VM thread in
            // the decoder; capture them before the Arc is moved into the worker.
            let provider_name = provider.name().to_string();
            // Capture the dispatch-time budget + leaf-usage frames (ASYNC-1), so the
            // decoder charges the frames active now, not whatever scope is installed
            // when the future lands. Mirrors the completion path.
            let usage_accum_slot = current_usage_accum();
            let budget_slot = active_budget();

            // A unified-runtime spawned task SUSPENDS on an External wait (the wire
            // call runs off the VM thread on the executor's async tier); the decoder
            // finalizes the span, cassette, and usage on the VM thread when it lands.
            if in_runtime_offload_task() {
                use sema_core::runtime::{
                    CompletionKind, InterruptibleResource, NativeSuspend,
                    PreparedExternalOperation, SendPayload, WaitKind,
                };
                let provider_for_job = provider.clone();
                let req_for_job = request.clone();
                let decoder = Box::new(EmbedDecoder {
                    span: Some(span),
                    provider_name: provider_name.clone(),
                    req_model: req_model.clone(),
                    recording,
                    key: cassette_key.clone(),
                    cassette_scope,
                    single,
                    usage_accum_slot: usage_accum_slot.clone(),
                    budget_slot: budget_slot.clone(),
                });
                let kind = CompletionKind::try_from_raw(EMBED_COMPLETION_KIND)
                    .expect("embed completion kind is nonzero");
                let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
                let resource = InterruptibleResource::new(
                    "llm/embed",
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
                            match provider_for_job.embed_future(req_for_job.clone()) {
                                Some(fut) => fut.await,
                                None => {
                                    let p = provider_for_job.clone();
                                    sema_io::io_offload_blocking(move || p.embed(req_for_job)).await
                                }
                            }
                        };
                        // A mid-flight cancel drops the in-flight request future.
                        let r = tokio::select! {
                            biased;
                            _ = cancel_rx => Err(LlmError::Config("cancelled".to_string())),
                            r = work => r,
                        };
                        Ok(Box::new(r) as SendPayload)
                    },
                );
                return Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::External(Box::new(prepared)),
                    continuation: Box::new(OffloadValueContinuation { op: "llm/embed" }),
                }));
            }
        }

        // Synchronous embedding calls bypass do_complete.
        let span = sema_otel::llm_span("embeddings");
        span.set_embedding_input(&request.texts);
        let decision = cassette_decide(&cassette_key);
        let response = match decision {
            Some(crate::cassette::Decision::Replay(entry)) => {
                enforce_stored_model_policy(&entry.provider, &entry.model, PolicySource::Cassette)?;
                let resp = EmbedResponse {
                    embeddings: entry.embeddings,
                    model: entry.model.clone(),
                    usage: Usage {
                        prompt_tokens: entry.prompt_tokens,
                        model: entry.model,
                        ..Default::default()
                    },
                };
                span.set_dispatch("cassette", &req_model);
                span.set_response(&sema_otel::ResponseFacts {
                    input_tokens: resp.usage.prompt_tokens,
                    output_tokens: 0,
                    response_model: resp.model.clone(),
                    ..Default::default()
                });
                resp
            }
            Some(crate::cassette::Decision::Miss(k)) => return Err(cassette_miss_error(&k)),
            other => {
                let recording = matches!(other, Some(crate::cassette::Decision::Record));
                let (resp, provider_name) = with_embedding_provider(|p| {
                    let model = request
                        .model
                        .as_deref()
                        .unwrap_or_else(|| p.default_model());
                    model_target_allowed(p.name(), model, PolicySource::Request, false)?;
                    let resp = match p.embed(request) {
                        Ok(r) => r,
                        Err(e) => {
                            span.record_error(llm_error_kind(&e), &e.to_string());
                            return Err(SemaError::Llm(e.to_string()));
                        }
                    };
                    span.set_dispatch(p.name(), &req_model);
                    span.set_response(&sema_otel::ResponseFacts {
                        input_tokens: resp.usage.prompt_tokens,
                        output_tokens: 0,
                        response_model: resp.model.clone(),
                        cost_usd: pricing::calculate_cost_for(p.name(), &resp.usage),
                        ..Default::default()
                    });
                    Ok((resp, p.name().to_string()))
                })?;
                if recording {
                    cassette_record(crate::cassette::TapeEntry::from_embed(
                        &cassette_key,
                        &provider_name,
                        &resp.model,
                        &resp.embeddings,
                        resp.usage.prompt_tokens,
                    ));
                }
                resp
            }
        };

        track_usage(&response.usage)?;
        Ok(NativeOutcome::Return(embed_value_from_response(
            &response, single,
        )))
    });

    // (llm/rerank query documents {:top-k 5 :model "..." :provider :cohere})
    // Cross-encoder reranking. Returns a list of {:index :score :document}, highest
    // relevance first. `documents` is a list of strings.
    register_runtime_fn_ctx(env, "llm/rerank", |_ctx, args| {
        #[allow(unused_imports)]
        use sema_core::runtime::NativeOutcome;
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/rerank", "2-3", args.len()));
        }
        let mut query = args[0]
            .as_str()
            .ok_or_else(|| SemaError::argument_type("llm/rerank", 1, "string query", &args[0]))?
            .to_string();
        let mut documents: Vec<String> = args[1]
            .as_seq()
            .ok_or_else(|| {
                SemaError::argument_type("llm/rerank", 2, "list or vector of strings", &args[1])
            })?
            .iter()
            .enumerate()
            .map(|(index, document)| {
                document.as_str().map(str::to_string).ok_or_else(|| {
                    SemaError::eval(format!(
                        "llm/rerank argument 2 entry {} must be a string, got {}",
                        index + 1,
                        document.type_name()
                    ))
                })
            })
            .collect::<Result<_, _>>()?;
        if documents.is_empty() {
            return Ok(NativeOutcome::Return(Value::list(vec![])));
        }
        query = apply_text_policy(
            &query,
            PolicyBoundary::LlmInput,
            "rerank.query",
            PolicySource::Request,
            None,
        )?;
        apply_input_policy_to_texts(&mut documents, "rerank.document")?;

        let mut top_k = None;
        let mut model = None;
        let mut provider = None;
        if let Some(options) = args.get(2) {
            let opts = options
                .as_map_rc()
                .ok_or_else(|| SemaError::argument_type("llm/rerank", 3, "map", options))?;
            top_k = opts
                .get(&Value::keyword("top-k"))
                .map(|value| {
                    value
                        .as_int()
                        .and_then(|value| usize::try_from(value).ok())
                        .filter(|value| *value > 0)
                        .ok_or_else(|| {
                            SemaError::eval(format!(
                                "llm/rerank option :top-k must be a positive integer, got {value}"
                            ))
                        })
                })
                .transpose()?;
            model = opts
                .get(&Value::keyword("model"))
                .map(|value| {
                    value.as_str().map(str::to_string).ok_or_else(|| {
                        SemaError::eval(format!(
                            "llm/rerank option :model must be a string, got {}",
                            value.type_name()
                        ))
                    })
                })
                .transpose()?;
            provider = opts
                .get(&Value::keyword("provider"))
                .map(|value| {
                    value
                        .as_keyword()
                        .or_else(|| value.as_str().map(str::to_string))
                        .ok_or_else(|| {
                            SemaError::eval(format!(
                                "llm/rerank option :provider must be a keyword or string, got {}",
                                value.type_name()
                            ))
                        })
                })
                .transpose()?;
        }

        let request = RerankRequest {
            query: query.clone(),
            documents: documents.clone(),
            top_k,
            model: model.clone(),
        };

        // ── Runtime path: offload and suspend (native targets only) ─────────
        //
        // The concurrent rerank path is native-only (no shared tokio runtime on
        // wasm), so wasm always falls through to the synchronous path below.
        #[cfg(not(target_arch = "wasm32"))]
        if in_runtime_offload_context() {
            // DETACHED reranker span: parent captured now, finalized by the
            // decoder after the wait (where the active-span stack may hold a
            // sibling task's span, so the span must not pop the stack on drop).
            let span =
                sema_otel::reranker_span_detached(&query, model.as_deref().unwrap_or(""), top_k);
            span.set_input(&documents);

            // Clone an Arc<provider> off the thread-local registry on THIS thread,
            // release the borrow, and move it into the offloaded future.
            let resolved_provider = PROVIDER_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                match provider.as_deref() {
                    Some(n) => reg
                        .get(n)
                        .ok_or_else(|| SemaError::Llm(format!("rerank provider '{n}' not found"))),
                    None => reg
                        .rerank_provider()
                        .or_else(|| reg.default_provider())
                        .ok_or_else(|| {
                            SemaError::Llm(
                            "no rerank provider configured — set COHERE_API_KEY, JINA_API_KEY, or \
                             VOYAGE_API_KEY (or pass {:provider ...})"
                                .to_string(),
                        )
                        }),
                }
            })?;
            let resolved_model = request
                .model
                .as_deref()
                .unwrap_or_else(|| resolved_provider.default_model());
            model_target_allowed(
                resolved_provider.name(),
                resolved_model,
                PolicySource::Request,
                false,
            )?;

            // Root-main and spawned tasks suspend on an External wait; the decoder
            // builds the reordered output on the VM thread when it lands.
            if in_runtime_offload_task() {
                use sema_core::runtime::{
                    CompletionKind, InterruptibleResource, NativeSuspend,
                    PreparedExternalOperation, SendPayload, WaitKind,
                };
                let provider_for_job = resolved_provider.clone();
                let req_for_job = request.clone();
                let decoder = Box::new(RerankDecoder {
                    span: Some(span),
                    documents: documents.clone(),
                });
                let kind = CompletionKind::try_from_raw(RERANK_COMPLETION_KIND)
                    .expect("rerank completion kind is nonzero");
                let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
                let resource = InterruptibleResource::new(
                    "llm/rerank",
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
                            match provider_for_job.rerank_future(req_for_job.clone()) {
                                Some(fut) => fut.await,
                                None => {
                                    let p = provider_for_job.clone();
                                    sema_io::io_offload_blocking(move || p.rerank(req_for_job))
                                        .await
                                }
                            }
                        };
                        // A mid-flight cancel drops the in-flight request future.
                        let r = tokio::select! {
                            biased;
                            _ = cancel_rx => Err(LlmError::Config("cancelled".to_string())),
                            r = work => r,
                        };
                        Ok(Box::new(r) as SendPayload)
                    },
                );
                return Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::External(Box::new(prepared)),
                    continuation: Box::new(OffloadValueContinuation { op: "llm/rerank" }),
                }));
            }
        }

        // OpenInference RERANKER span (no-op unless telemetry + compat are on).
        let span = sema_otel::reranker_span(&query, model.as_deref().unwrap_or(""), top_k);
        span.set_input(&documents);

        let resp = with_rerank_provider(provider.as_deref(), |p| {
            let resolved_model = request
                .model
                .as_deref()
                .unwrap_or_else(|| p.default_model());
            model_target_allowed(p.name(), resolved_model, PolicySource::Request, false)?;
            p.rerank(request).map_err(|e| {
                span.record_error(llm_error_kind(&e), &e.to_string());
                SemaError::Llm(e.to_string())
            })
        })?;

        // Record reordered output (content + scores) on the span.
        let out_docs: Vec<(String, f64)> = resp
            .results
            .iter()
            .filter_map(|r| documents.get(r.index).map(|d| (d.clone(), r.score)))
            .collect();
        span.set_output(&out_docs);

        Ok(NativeOutcome::Return(rerank_value_from_response(
            &resp, &documents,
        )))
    });

    // (llm/similarity vec1 vec2) — cosine similarity
    register_fn(env, "llm/similarity", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/similarity", "2", args.len()));
        }

        let a_is_bv = args[0].as_bytevector().is_some();
        let b_is_bv = args[1].as_bytevector().is_some();
        let a_is_list = args[0].as_seq().is_some();
        let b_is_list = args[1].as_seq().is_some();

        if a_is_bv && b_is_bv {
            let ba = args[0].as_bytevector().unwrap();
            let bb = args[1].as_bytevector().unwrap();
            if ba.len() != bb.len() {
                return Err(SemaError::eval(format!(
                    "llm/similarity: bytevectors must have same length ({} vs {})",
                    ba.len(),
                    bb.len()
                )));
            }
            if ba.is_empty() {
                return Err(SemaError::eval("llm/similarity: empty vectors"));
            }
            if ba.len() % 8 != 0 {
                return Err(SemaError::eval(format!(
                    "llm/similarity: bytevector length must be a multiple of 8 (got {})",
                    ba.len()
                )));
            }
            let mut dot = 0.0_f64;
            let mut mag_a = 0.0_f64;
            let mut mag_b = 0.0_f64;
            for (ca, cb) in ba.as_chunks::<8>().0.iter().zip(bb.as_chunks::<8>().0) {
                let fa = f64::from_le_bytes(*ca);
                let fb = f64::from_le_bytes(*cb);
                dot += fa * fb;
                mag_a += fa * fa;
                mag_b += fb * fb;
            }
            if mag_a == 0.0 || mag_b == 0.0 {
                Ok(Value::float(0.0))
            } else {
                Ok(Value::float(dot / (mag_a.sqrt() * mag_b.sqrt())))
            }
        } else if a_is_list && b_is_list {
            let va = extract_float_vec(&args[0])?;
            let vb = extract_float_vec(&args[1])?;
            if va.len() != vb.len() {
                return Err(SemaError::eval(format!(
                    "llm/similarity: vectors must have same length ({} vs {})",
                    va.len(),
                    vb.len()
                )));
            }
            if va.is_empty() {
                return Err(SemaError::eval("llm/similarity: empty vectors"));
            }
            let mut dot = 0.0_f64;
            let mut mag_a = 0.0_f64;
            let mut mag_b = 0.0_f64;
            for i in 0..va.len() {
                dot += va[i] * vb[i];
                mag_a += va[i] * va[i];
                mag_b += vb[i] * vb[i];
            }
            if mag_a == 0.0 || mag_b == 0.0 {
                Ok(Value::float(0.0))
            } else {
                Ok(Value::float(dot / (mag_a.sqrt() * mag_b.sqrt())))
            }
        } else {
            Err(SemaError::eval(
                "llm/similarity: both arguments must be the same type (both bytevectors or both lists). \
                 Use embedding/->list or embedding/list->embedding to convert between formats.",
            ))
        }
    });

    register_fn(env, "embedding/length", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("embedding/length", "1", args.len()));
        }
        let bv = args.bytes_at(0, "embedding/length")?;
        if bv.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "embedding/length: bytevector length {} is not divisible by 8",
                bv.len()
            )));
        }
        Ok(Value::int((bv.len() / 8) as i64))
    });

    register_fn(env, "embedding/ref", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("embedding/ref", "2", args.len()));
        }
        let bv = args.bytes_at(0, "embedding/ref")?;
        let idx = args.int_at(1, "embedding/ref")?;
        if bv.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "embedding/ref: bytevector length {} is not divisible by 8",
                bv.len()
            )));
        }
        let idx = idx as usize;
        let num_elements = bv.len() / 8;
        if idx >= num_elements {
            return Err(SemaError::eval(format!(
                "embedding/ref: index {} out of bounds (length {})",
                idx, num_elements
            )));
        }
        let start = idx * 8;
        let bytes: [u8; 8] = bv[start..start + 8].try_into().unwrap();
        Ok(Value::float(f64::from_le_bytes(bytes)))
    });

    register_fn(env, "embedding/->list", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("embedding/->list", "1", args.len()));
        }
        let bv = args.bytes_at(0, "embedding/->list")?;
        if bv.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "embedding/->list: bytevector length {} is not divisible by 8",
                bv.len()
            )));
        }
        let floats: Vec<Value> = bv
            .as_chunks::<8>()
            .0
            .iter()
            .map(|chunk| Value::float(f64::from_le_bytes(*chunk)))
            .collect();
        Ok(Value::list(floats))
    });

    register_fn(env, "embedding/list->embedding", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity(
                "embedding/list->embedding",
                "1",
                args.len(),
            ));
        }
        let items = args.seq_at(0, "embedding/list->embedding")?;
        let mut bytes = Vec::with_capacity(items.len() * 8);
        for (i, item) in items.iter().enumerate() {
            let f = item.as_float().ok_or_else(|| {
                SemaError::eval(format!(
                    "embedding/list->embedding: element {} is {}, expected number",
                    i,
                    item.type_name()
                ))
            })?;
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        Ok(Value::bytevector(bytes))
    });
}
