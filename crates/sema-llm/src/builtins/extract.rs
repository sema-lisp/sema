use super::*;

pub(super) fn format_reask_prompt(prev_response: &str, errors: &str, schema_desc: &str) -> String {
    format!(
        "Your previous response did not match the required schema.\n\n\
         Previous response:\n```json\n{prev_response}\n```\n\n\
         Validation errors:\n{errors}\n\n\
         Please respond with ONLY a corrected JSON object matching this schema:\n\
         {schema_desc}\nDo not include any other text."
    )
}

pub(super) fn format_schema(val: &Value) -> String {
    if let Some(map) = val.as_map_rc() {
        let mut fields = Vec::new();
        for (k, v) in map.iter() {
            let key = k
                .as_keyword()
                .or_else(|| k.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| k.to_string());
            let type_str = if let Some(inner) = v.as_map_rc() {
                if let Some(t) = inner.get(&Value::keyword("type")) {
                    t.as_keyword()
                        .or_else(|| t.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| t.to_string())
                } else {
                    "any".to_string()
                }
            } else if let Some(kw) = v.as_keyword() {
                // Bare keyword spec ({:total :number}) is shorthand for
                // {:type :number} — tell the model the type instead of <any>.
                kw
            } else {
                "any".to_string()
            };
            fields.push(format!("  \"{key}\": <{type_str}>"));
        }
        format!("{{\n{}\n}}", fields.join(",\n"))
    } else {
        val.to_string()
    }
}

/// Validate that an extracted Sema value matches the expected schema.
/// The schema is a map of keyword keys to field descriptors (maps with `:type`).
/// Returns Ok(()) if valid, or Err with a description of mismatches.
pub(super) fn validate_extraction(result: &Value, schema: &Value) -> Result<(), String> {
    let mut errors = Vec::new();
    for step in prepare_extraction_validation(result, schema) {
        match step {
            ExtractionValidationStep::Error(error) => errors.push(error),
            ExtractionValidationStep::Predicate {
                callable,
                argument,
                key_name,
                failure_message,
            } => match sema_core::with_stdlib_ctx(|ctx| {
                sema_core::call_callback(ctx, &callable, std::slice::from_ref(&argument))
            }) {
                Ok(value) if value.is_truthy() => {}
                Ok(_) => errors.push(format!("key {key_name}: {failure_message}")),
                Err(error) => {
                    errors.push(format!("key {key_name}: validation error: {error}"));
                }
            },
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub(super) enum ExtractionValidationStep {
    Error(String),
    Predicate {
        callable: Value,
        argument: Value,
        key_name: String,
        failure_message: String,
    },
}

/// Produce validation work in schema order. Runtime callers drive predicates as
/// structural calls; synchronous callers consume the same sequence inline.
pub(super) fn prepare_extraction_validation(
    result: &Value,
    schema: &Value,
) -> VecDeque<ExtractionValidationStep> {
    let schema_map = match schema.as_map_rc() {
        Some(m) => m,
        None => return VecDeque::new(),
    };
    let result_map = match result.as_map_rc() {
        Some(m) => m,
        None => {
            return VecDeque::from([ExtractionValidationStep::Error(format!(
                "expected map result, got {}",
                result.type_name()
            ))]);
        }
    };

    let mut steps = VecDeque::new();

    for (key, field_spec) in schema_map.iter() {
        let key_name = key
            .as_keyword()
            .or_else(|| key.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| key.to_string());

        // Check if field is optional (only applies to map-style field specs).
        // A declared :default also excuses the caller from supplying the key —
        // that is the point of a default.
        let is_optional = if let Some(spec) = field_spec.as_map_rc() {
            spec.get(&Value::keyword("optional"))
                .map(|v| v.is_truthy())
                .unwrap_or(false)
                || spec.get(&Value::keyword("default")).is_some()
        } else {
            false
        };

        let result_val = result_map.get(key);
        match result_val {
            None => {
                if !is_optional {
                    steps.push_back(ExtractionValidationStep::Error(format!(
                        "missing key: {key_name}"
                    )));
                }
            }
            Some(val) => {
                // A bare keyword spec ({:total :number}) is shorthand for
                // {:type :number} — it must be type-checked too, not skipped.
                if let Some(type_name) = field_spec.as_keyword() {
                    let ok = match type_name.as_str() {
                        "string" => val.as_str().is_some(),
                        "number" => val.as_float().is_some(),
                        "boolean" | "bool" => val.as_bool().is_some(),
                        "list" | "array" => val.as_seq().is_some(),
                        _ => true,
                    };
                    if !ok {
                        steps.push_back(ExtractionValidationStep::Error(format!(
                            "key {key_name}: expected {type_name}, got {}",
                            val.type_name()
                        )));
                    }
                } else if let Some(spec) = field_spec.as_map_rc() {
                    // Type checking
                    if let Some(type_val) = spec.get(&Value::keyword("type")) {
                        let type_name = type_val
                            .as_keyword()
                            .or_else(|| type_val.as_str().map(|s| s.to_string()))
                            .unwrap_or_else(|| type_val.to_string());
                        let ok = match type_name.as_str() {
                            "string" => val.as_str().is_some(),
                            "number" => val.as_float().is_some(),
                            "boolean" | "bool" => val.as_bool().is_some(),
                            "list" | "array" => val.as_seq().is_some(),
                            _ => true,
                        };
                        if !ok {
                            steps.push_back(ExtractionValidationStep::Error(format!(
                                "key {key_name}: expected {type_name}, got {}",
                                val.type_name()
                            )));
                            continue; // skip :validate if type check failed
                        }
                    }

                    // Custom predicate validation via :validate
                    if let Some(validate_fn) = spec.get(&Value::keyword("validate")) {
                        let failure_message = spec
                            .get(&Value::keyword("message"))
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                            .unwrap_or_else(|| format!("custom validation failed for value {val}"));
                        steps.push_back(ExtractionValidationStep::Predicate {
                            callable: validate_fn.clone(),
                            argument: val.clone(),
                            key_name,
                            failure_message,
                        });
                    }
                }
            }
        }
    }

    steps
}

/// Parameters shared by every `llm/extract` validation and re-ask attempt.
pub(super) struct ExtractConfig {
    pub(super) schema: Value,
    pub(super) schema_desc: String,
    pub(super) system: String,
    pub(super) model: String,
    pub(super) messages: Vec<ChatMessage>,
    pub(super) validate: bool,
    pub(super) max_retries: u32,
    pub(super) reask: bool,
}

pub(super) fn extract_reask_request(
    cfg: &ExtractConfig,
    last_response_content: &str,
    last_validation_error: &str,
) -> ChatRequest {
    let mut request = ChatRequest::new(cfg.model.clone(), cfg.messages.clone());
    request.json_mode = true;
    request.system = Some(if cfg.reask {
        format_reask_prompt(
            last_response_content,
            last_validation_error,
            &cfg.schema_desc,
        )
    } else {
        format!(
            "{}\n\nYour previous response had validation errors: {}. Please fix.",
            cfg.system, last_validation_error
        )
    });
    request
}

/// Parse an LLM extraction response body into a Sema `Value` (strips a ```json
/// fence if present). Shared by every `llm/extract` attempt.
pub(super) fn extract_parse_response(response: &ChatResponse) -> Result<Value, SemaError> {
    let content = response.content.trim();
    let json_str = if content.starts_with("```") {
        content
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
    } else {
        content
    };
    let json: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        SemaError::Llm(format!(
            "failed to parse LLM JSON response: {e}\nResponse was: {content}"
        ))
    })?;
    Ok(sema_core::json_to_value(&json))
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) enum RuntimeExtractPhase {
    Ready,
    Predicate {
        key_name: String,
        failure_message: String,
    },
}

/// Reinstall the extraction task's dispatch-time dynamic scopes while preparing
/// a later completion round. Native continuations resume outside the runtime's
/// per-quantum scope swap, so recursive provider preparation must supply the
/// captured LLM, telemetry, and leaf-accounting contexts explicitly.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct RuntimeCompletionPrepScope {
    previous_llm: Option<LlmDynScope>,
    previous_otel: Option<sema_otel::OtelTaskCtx>,
    previous_usage: Option<Option<Rc<RefCell<LeafUsage>>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeCompletionPrepScope {
    fn install(
        llm: LlmDynScope,
        otel: sema_otel::OtelTaskCtx,
        usage: Option<Rc<RefCell<LeafUsage>>>,
    ) -> Self {
        let previous_llm = write_llm_scope(llm);
        let previous_otel = sema_otel::install_task_otel(otel);
        let previous_usage =
            ACTIVE_LEAF_SCOPE.with(|active| std::mem::replace(&mut *active.borrow_mut(), usage));
        Self {
            previous_llm: Some(previous_llm),
            previous_otel: Some(previous_otel),
            previous_usage: Some(previous_usage),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for RuntimeCompletionPrepScope {
    fn drop(&mut self) {
        if let Some(previous_usage) = self.previous_usage.take() {
            ACTIVE_LEAF_SCOPE.with(|active| *active.borrow_mut() = previous_usage);
        }
        if let Some(previous_otel) = self.previous_otel.take() {
            let _ = sema_otel::install_task_otel(previous_otel);
        }
        if let Some(previous_llm) = self.previous_llm.take() {
            let _ = write_llm_scope(previous_llm);
        }
    }
}

/// Cooperative validation and re-ask state for `llm/extract`. Provider rounds
/// reuse the canonical completion driver, and custom validators are structural
/// VM calls so either side may suspend without occupying the VM thread.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct RuntimeExtractDriver {
    pub(super) cfg: ExtractConfig,
    pub(super) attempt: u32,
    pub(super) last_validation_error: String,
    pub(super) last_response_content: String,
    pub(super) result: Option<Value>,
    pub(super) steps: VecDeque<ExtractionValidationStep>,
    pub(super) errors: Vec<String>,
    pub(super) phase: RuntimeExtractPhase,
    pub(super) llm_scope: LlmDynScope,
    pub(super) otel_scope: sema_otel::OtelTaskCtx,
    pub(super) usage_accum_slot: Option<Rc<RefCell<LeafUsage>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeExtractDriver {
    fn new(cfg: ExtractConfig) -> Self {
        Self {
            cfg,
            attempt: 0,
            last_validation_error: String::new(),
            last_response_content: String::new(),
            result: None,
            steps: VecDeque::new(),
            errors: Vec::new(),
            phase: RuntimeExtractPhase::Ready,
            llm_scope: read_llm_scope(),
            otel_scope: sema_otel::current_conversation_scope(),
            usage_accum_slot: current_usage_accum(),
        }
    }

    fn accept_response(
        mut self: Box<Self>,
        response: ChatResponse,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::NativeOutcome;

        self.last_response_content = response.content.trim().to_string();
        let result = extract_parse_response(&response)?;
        if !self.cfg.validate {
            return Ok(NativeOutcome::Return(result));
        }

        self.steps = prepare_extraction_validation(&result, &self.cfg.schema);
        self.result = Some(result);
        self.errors.clear();
        self.phase = RuntimeExtractPhase::Ready;
        self.advance_validation()
    }

    fn advance_validation(mut self: Box<Self>) -> sema_core::runtime::NativeResult {
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
                    self.phase = RuntimeExtractPhase::Predicate {
                        key_name,
                        failure_message,
                    };
                    return Ok(NativeOutcome::Call(NativeCall {
                        callable,
                        args: vec![argument],
                        continuation: self,
                    }));
                }
            }
        }

        if self.errors.is_empty() {
            let result = self
                .result
                .take()
                .ok_or_else(|| SemaError::eval("llm/extract validation lost its parsed result"))?;
            return Ok(NativeOutcome::Return(result));
        }

        self.last_validation_error = std::mem::take(&mut self.errors).join("; ");
        if self.attempt == self.cfg.max_retries {
            return Err(SemaError::Llm(format!(
                "extraction validation failed after {} attempt(s): {}",
                self.cfg.max_retries + 1,
                self.last_validation_error
            )));
        }

        self.attempt += 1;
        self.result = None;
        self.phase = RuntimeExtractPhase::Ready;
        let request = extract_reask_request(
            &self.cfg,
            &self.last_response_content,
            &self.last_validation_error,
        );
        let _scope = RuntimeCompletionPrepScope::install(
            self.llm_scope.clone(),
            self.otel_scope.clone(),
            self.usage_accum_slot.clone(),
        );
        do_complete_runtime_suspend(request, CompleteFinalize::runtime(self))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for RuntimeExtractDriver {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        sink(sema_core::cycle::GcEdge::Value(&self.cfg.schema));
        if let Some(result) = &self.result {
            sink(sema_core::cycle::GcEdge::Value(result));
        }
        for step in &self.steps {
            if let ExtractionValidationStep::Predicate {
                callable, argument, ..
            } = step
            {
                sink(sema_core::cycle::GcEdge::Value(callable));
                sink(sema_core::cycle::GcEdge::Value(argument));
            }
        }
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl CompleteResponseContinuation for RuntimeExtractDriver {
    fn finish(self: Box<Self>, response: ChatResponse) -> sema_core::runtime::NativeResult {
        self.accept_response(response)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::NativeContinuation for RuntimeExtractDriver {
    fn resume(
        mut self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::ResumeInput;

        let phase = std::mem::replace(&mut self.phase, RuntimeExtractPhase::Ready);
        match (phase, input) {
            (
                RuntimeExtractPhase::Predicate {
                    key_name,
                    failure_message,
                },
                ResumeInput::Returned(value),
            ) => {
                if !value.is_truthy() {
                    self.errors
                        .push(format!("key {key_name}: {failure_message}"));
                }
                self.advance_validation()
            }
            (RuntimeExtractPhase::Predicate { key_name, .. }, ResumeInput::Failed(error)) => {
                self.errors
                    .push(format!("key {key_name}: validation error: {error}"));
                self.advance_validation()
            }
            (_, ResumeInput::Cancelled(reason)) => Err(SemaError::eval(format!(
                "llm/extract validator was cancelled ({reason:?})"
            ))),
            (_, ResumeInput::Runtime(_)) => Err(SemaError::eval(
                "llm/extract validator received an unexpected runtime response",
            )),
            (RuntimeExtractPhase::Ready, _) => Err(SemaError::eval(
                "llm/extract validator resumed without an active predicate",
            )),
        }
    }
}

/// Validate `llm/extract` synchronously for host calls and wasm builds. `first` is
/// the attempt-0 response and has already been accounted by the caller.
pub(super) fn extract_validate_and_reask(
    first: ChatResponse,
    cfg: &ExtractConfig,
) -> Result<Value, SemaError> {
    let mut last_validation_error = String::new();
    let mut last_response_content = String::new();

    for attempt in 0..=cfg.max_retries {
        // Attempt 0 reuses the already-issued+accounted `first` response; later
        // attempts issue a fresh (re-ask) request synchronously here.
        let response = if attempt == 0 {
            first.clone()
        } else {
            let request =
                extract_reask_request(cfg, &last_response_content, &last_validation_error);
            let resp = do_complete(request)?;
            track_usage(&resp.usage)?;
            resp
        };

        let content = response.content.trim().to_string();
        let result = extract_parse_response(&response)?;

        if !cfg.validate {
            return Ok(result);
        }
        match validate_extraction(&result, &cfg.schema) {
            Ok(()) => return Ok(result),
            Err(err) => {
                last_validation_error = err;
                last_response_content = content;
                if attempt == cfg.max_retries {
                    return Err(SemaError::Llm(format!(
                        "extraction validation failed after {} attempt(s): {}",
                        cfg.max_retries + 1,
                        last_validation_error
                    )));
                }
            }
        }
    }

    unreachable!()
}

pub(super) fn register(env: &Env, sandbox: &sema_core::Sandbox) {
    // (llm/extract schema text {:model "..." :validate true :retries 2 :reask? true})
    register_runtime_fn_ctx(env, "llm/extract", |_ctx, args| {
        #[allow(unused_imports)]
        use sema_core::runtime::NativeOutcome;
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/extract", "2-3", args.len()));
        }
        let schema = args[0].clone();
        let text = args.str_at(1, "llm/extract")?;

        let schema_desc = format_schema(&schema);
        let system = format!(
            "Extract structured data from the text. Respond with ONLY a JSON object matching this schema:\n{}\nDo not include any other text.",
            schema_desc
        );
        let messages = vec![ChatMessage::new("user", text)];

        let mut model = String::new();
        let mut validate = true;
        let mut max_retries: u32 = 2;
        let mut reask = true;
        if let Some(opts_val) = args.get(2) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = opts.opt_str("model").unwrap_or_default();
                if let Some(v) = opts.get(&Value::keyword("validate")) {
                    validate = v.is_truthy();
                }
                if let Some(r) = opts.opt_int("retries").map(|n| n as u32) {
                    max_retries = r;
                }
                if let Some(v) = opts.get(&Value::keyword("reask?")) {
                    reask = v.is_truthy();
                }
            }
        }

        // Attempt 0: the initial extraction request.
        let mut request = ChatRequest::new(model.clone(), messages.clone());
        request.json_mode = true;
        request.system = Some(system.clone());

        let cfg = ExtractConfig {
            schema,
            schema_desc,
            system,
            model,
            messages,
            validate,
            max_retries,
            reask,
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            if in_runtime_offload_task() {
                return do_complete_runtime_suspend(
                    request,
                    CompleteFinalize::runtime(Box::new(RuntimeExtractDriver::new(cfg))),
                );
            }
            let first = do_complete(request)?;
            track_usage(&first.usage)?;
            extract_validate_and_reask(first, &cfg).map(NativeOutcome::Return)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let first = do_complete(request)?;
            track_usage(&first.usage)?;
            extract_validate_and_reask(first, &cfg).map(NativeOutcome::Return)
        }
    });

    // (llm/extract-from-image schema source {:model "..."})
    // source: string path or bytevector
    register_runtime_fn_ctx_gated_as(
        env,
        sandbox,
        sema_core::Caps::LLM,
        "llm/extract-from-image",
        "llm/extract-from-image",
        |_ctx, args| {
            #[allow(unused_imports)]
            use sema_core::runtime::NativeOutcome;
            if args.len() < 2 || args.len() > 3 {
                return Err(SemaError::arity(
                    "llm/extract-from-image",
                    "2-3",
                    args.len(),
                ));
            }
            let schema = &args[0];

            // Get image bytes: either from path (string) or bytevector
            let bytes = if let Some(path) = args[1].as_str() {
                std::fs::read(path).io_ctx(format!("llm/extract-from-image: {path}"))?
            } else if let Some(bv) = args[1].as_bytevector() {
                bv.to_vec()
            } else {
                return Err(SemaError::type_error(
                    "string path or bytevector",
                    args[1].type_name(),
                ));
            };

            let media_type = detect_media_type(&bytes).to_string();
            use base64::Engine;
            let b64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

            let schema_desc = format_schema(schema);
            let system = format!(
                "Extract structured data from the image. Respond with ONLY a JSON object matching this schema:\n{}\nDo not include any other text.",
                schema_desc
            );

            let messages = vec![ChatMessage::with_blocks(
                "user",
                vec![
                    ContentBlock::Image {
                        media_type: Some(media_type),
                        data: b64_data,
                    },
                    ContentBlock::Text {
                        text: "Extract the requested data from this image. Respond in JSON."
                            .to_string(),
                    },
                ],
            )];

            let mut model = String::new();
            if let Some(opts_val) = args.get(2) {
                if let Some(opts) = opts_val.as_map_rc() {
                    model = opts.opt_str("model").unwrap_or_default();
                }
            }

            let mut request = ChatRequest::new(model, messages);
            request.system = Some(system);
            request.json_mode = true;

            #[cfg(not(target_arch = "wasm32"))]
            {
                dispatch_complete_offload(
                    request,
                    CompleteFinalize::new(|response| extract_parse_response(&response)),
                )
            }
            #[cfg(target_arch = "wasm32")]
            {
                let response = do_complete(request)?;
                track_usage(&response.usage)?;
                extract_parse_response(&response).map(NativeOutcome::Return)
            }
        },
    );

    // (llm/classify categories text {:model "..."})
    register_runtime_fn_ctx(env, "llm/classify", |_ctx, args| {
        #[allow(unused_imports)]
        use sema_core::runtime::NativeOutcome;
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/classify", "2-3", args.len()));
        }
        let categories = args.seq_at(0, "llm/classify")?.to_vec();
        let text = args.str_at(1, "llm/classify")?;

        let cat_names: Vec<String> = categories
            .iter()
            .map(|c| {
                if let Some(kw) = c.as_keyword() {
                    kw
                } else if let Some(s) = c.as_str() {
                    s.to_string()
                } else {
                    c.to_string()
                }
            })
            .collect();

        let system = format!(
            "Classify the following text into exactly one of these categories: {}\nRespond with ONLY the category name, nothing else.",
            cat_names.join(", ")
        );
        let messages = vec![ChatMessage::new("user", text)];

        let mut model = String::new();
        if let Some(opts_val) = args.get(2) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = opts.opt_str("model").unwrap_or_default();
            }
        }

        let mut request = ChatRequest::new(model, messages);
        request.system = Some(system);

        // Shape the response into a category keyword (if it matched a keyword in the
        // original list) or string. Shared by the sync and async paths.
        #[cfg(not(target_arch = "wasm32"))]
        let finalize_values = categories.clone();
        let parse_category = move |response: ChatResponse| -> Result<Value, SemaError> {
            let category = response.content.trim().to_string();
            if categories
                .iter()
                .any(|c| c.as_keyword().map(|kw| kw == category).unwrap_or(false))
            {
                Ok(Value::keyword(&category))
            } else {
                Ok(Value::string(&category))
            }
        };

        // Runtime roots and spawned tasks suspend on an External wait. The
        // completion decoder accounts usage and runs `parse_category`.
        #[cfg(not(target_arch = "wasm32"))]
        {
            dispatch_complete_offload(
                request,
                CompleteFinalize::with_values(parse_category, finalize_values),
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            let response = do_complete(request)?;
            track_usage(&response.usage)?;
            parse_category(response).map(NativeOutcome::Return)
        }
    });

    // Conversation functions
}
