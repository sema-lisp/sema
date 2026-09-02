use super::*;

pub(super) struct LispProviderCallbacks {
    pub(super) complete_fn: Value,
}

/// A provider defined in Sema code via lambdas.
/// Only stores String fields (Send+Sync); callbacks live in the
/// LISP_PROVIDERS thread-local, accessed only from the same thread.
pub(super) struct LispProvider {
    pub(super) name: String,
    pub(super) default_model: String,
}

pub(super) fn lisp_provider_complete_callback(name: &str) -> Result<Value, LlmError> {
    LISP_PROVIDERS.with(|providers| {
        providers
            .borrow()
            .get(name)
            .map(|callbacks| callbacks.complete_fn.clone())
            .ok_or_else(|| LlmError::Config(format!("lisp provider '{name}' callbacks not found")))
    })
}

impl LlmProvider for LispProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    // A Lisp provider's `:complete` closure runs on the VM thread via the
    // callback context, so it must never be offloaded to a pool worker.
    fn runs_on_vm_thread(&self) -> bool {
        true
    }

    fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let complete_fn = lisp_provider_complete_callback(&self.name)?;
        let request_map = chat_request_to_value(&request);

        // The LlmProvider trait gives us no caller ctx, so invoke the user's
        // `:complete` function on the shared stdlib context, which carries the
        // registered evaluator callback (same path stdlib HOFs use).
        let result = sema_core::with_stdlib_ctx(|ctx| {
            sema_core::call_callback(ctx, &complete_fn, &[request_map])
        });

        match result {
            Ok(response_val) => parse_lisp_provider_response(&response_val, &request.model),
            Err(e) => Err(LlmError::Api {
                status: 0,
                message: e.to_string(),
            }),
        }
    }
}

/// Convert a ChatRequest into a Sema Value::Map for passing to Lisp provider callbacks.
pub(super) fn chat_request_to_value(request: &ChatRequest) -> Value {
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("model"), Value::string(&request.model));

    let msgs: Vec<Value> = request
        .messages
        .iter()
        .map(|m| {
            let mut msg_map = BTreeMap::new();
            msg_map.insert(Value::keyword("role"), Value::string(&m.role));
            msg_map.insert(
                Value::keyword("content"),
                Value::string(&m.content.to_text()),
            );
            Value::map(msg_map)
        })
        .collect();
    map.insert(Value::keyword("messages"), Value::list(msgs));

    if let Some(max_tokens) = request.max_tokens {
        map.insert(Value::keyword("max-tokens"), Value::int(max_tokens as i64));
    }
    if let Some(temp) = request.temperature {
        map.insert(Value::keyword("temperature"), Value::float(temp));
    }
    if let Some(ref system) = request.system {
        map.insert(Value::keyword("system"), Value::string(system));
    }

    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                let mut tool_map = BTreeMap::new();
                tool_map.insert(Value::keyword("name"), Value::string(&t.name));
                tool_map.insert(Value::keyword("description"), Value::string(&t.description));
                tool_map.insert(
                    Value::keyword("parameters"),
                    sema_core::json_to_value(&t.parameters),
                );
                Value::map(tool_map)
            })
            .collect();
        map.insert(Value::keyword("tools"), Value::list(tools));
    }

    if !request.stop_sequences.is_empty() {
        let seqs: Vec<Value> = request
            .stop_sequences
            .iter()
            .map(|s| Value::string(s))
            .collect();
        map.insert(Value::keyword("stop-sequences"), Value::list(seqs));
    }

    Value::map(map)
}

/// Parse a Sema Value returned by a Lisp provider callback into a ChatResponse.
pub(super) fn parse_lisp_provider_response(
    val: &Value,
    model: &str,
) -> Result<ChatResponse, LlmError> {
    match val.view() {
        ValueView::String(s) => Ok(ChatResponse {
            content: s.to_string(),
            role: "assistant".to_string(),
            model: model.to_string(),
            tool_calls: vec![],
            usage: Usage {
                model: model.to_string(),
                ..Usage::default()
            },
            stop_reason: Some("end_turn".to_string()),
        }),
        ValueView::Map(map) => {
            let content = map
                .get(&Value::keyword("content"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let role = map
                .get(&Value::keyword("role"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "assistant".to_string());
            let resp_model = map
                .get(&Value::keyword("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| model.to_string());
            let stop_reason = map
                .get(&Value::keyword("stop-reason"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or(Some("end_turn".to_string()));

            let usage = if let Some(usage_val) = map.get(&Value::keyword("usage")) {
                if let Some(usage_map) = usage_val.as_map_rc() {
                    let prompt_tokens = usage_map
                        .get(&Value::keyword("prompt-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    let completion_tokens = usage_map
                        .get(&Value::keyword("completion-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    let cache_read_input_tokens = usage_map
                        .get(&Value::keyword("cache-read-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    let cache_creation_input_tokens = usage_map
                        .get(&Value::keyword("cache-creation-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    Usage {
                        prompt_tokens,
                        completion_tokens,
                        model: resp_model.clone(),
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    }
                } else {
                    Usage {
                        model: resp_model.clone(),
                        ..Default::default()
                    }
                }
            } else {
                Usage {
                    model: resp_model.clone(),
                    ..Default::default()
                }
            };

            let tool_calls = if let Some(tcs_val) = map.get(&Value::keyword("tool-calls")) {
                if let Some(tcs) = tcs_val.as_seq() {
                    tcs.iter()
                        .filter_map(|tc| {
                            let tc_map = tc.as_map_rc()?;
                            let id = tc_map
                                .get(&Value::keyword("id"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .unwrap_or_default();
                            let name = tc_map
                                .get(&Value::keyword("name"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())?;
                            let arguments = tc_map
                                .get(&Value::keyword("arguments"))
                                .map(sema_core::value_to_json_lossy)
                                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                            Some(ToolCall {
                                id,
                                name,
                                arguments,
                                thought_signature: tc_map
                                    .get(&Value::keyword("thought-signature"))
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                            })
                        })
                        .collect()
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

            Ok(ChatResponse {
                content,
                role,
                model: resp_model,
                tool_calls,
                usage,
                stop_reason,
            })
        }
        _ => Err(LlmError::Parse(
            "lisp provider must return a string or map with :content".to_string(),
        )),
    }
}
