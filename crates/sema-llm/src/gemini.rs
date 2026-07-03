use crate::provider::LlmProvider;
use crate::types::{ChatRequest, ChatResponse, LlmError, ToolCall, Usage};

/// Map canonical reasoning effort → Gemini `thinkingBudget` (tokens). `none`/
/// `minimal` disable thinking (0); others scale within the 2.5-flash 0..=24576
/// range. Unrecognized values default to disabled.
fn gemini_thinking_budget(effort: &str) -> u32 {
    match effort.to_lowercase().as_str() {
        "low" => 1024,
        "medium" => 8192,
        "high" | "xhigh" | "max" => 24576,
        _ => 0, // none / minimal / unrecognized
    }
}

/// Build the Gemini endpoint URL without embedding the API key (the key is sent
/// via the `x-goog-api-key` header instead — see the call sites). Validates the
/// request-controlled `model` so it cannot inject extra path segments or break
/// out of the `/models/<model>:<action>` shape (SSRF / path injection).
fn build_url(base_url: &str, model: &str, action: &str) -> Result<String, LlmError> {
    if model.is_empty()
        || model
            .chars()
            .any(|c| c == '/' || c == '?' || c == '#' || c == ':' || c.is_control())
        || model.contains("..")
    {
        return Err(LlmError::Config(format!("invalid model name: {model:?}")));
    }
    Ok(format!("{base_url}/models/{model}:{action}"))
}

pub struct GeminiProvider {
    api_key: String,
    base_url: String,
    default_model: String,
    client: reqwest::Client,
}

impl GeminiProvider {
    pub fn new(api_key: String, default_model: Option<String>) -> Result<Self, LlmError> {
        Ok(GeminiProvider {
            api_key,
            base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
            default_model: default_model.unwrap_or_else(|| "gemini-3.5-flash".to_string()),
            client: crate::http::create_client(None)?,
        })
    }

    fn resolve_model(&self, model: &str) -> String {
        if model.is_empty() {
            self.default_model.clone()
        } else {
            model.to_string()
        }
    }

    async fn complete_async(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let model = self.resolve_model(&request.model);
        let url = build_url(&self.base_url, &model, "generateContent")?;

        let body = self.build_request_body(&request);

        let resp = crate::http::with_timeout(self.client.post(&url), request.timeout_ms)
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let status = resp.status().as_u16();
        if status == 429 {
            return Err(LlmError::RateLimited {
                retry_after_ms: 5000,
            });
        }
        if status != 200 {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }

        let api_resp: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        self.parse_response(&api_resp, &model)
    }

    async fn stream_complete_async(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<ChatResponse, LlmError> {
        let model = self.resolve_model(&request.model);
        let url = format!(
            "{}?alt=sse",
            build_url(&self.base_url, &model, "streamGenerateContent")?
        );

        let body = self.build_request_body(&request);

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let status = resp.status().as_u16();
        if status == 429 {
            return Err(LlmError::RateLimited {
                retry_after_ms: 5000,
            });
        }
        if status != 200 {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }

        let mut full_content = String::new();
        let mut prompt_tokens = 0u32;
        let mut completion_tokens = 0u32;
        let mut cached_tokens = 0u32;
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tool_call_idx = 0usize;

        crate::sse::parse_sse_stream(resp, |data| {
            if let Ok(chunk) = serde_json::from_str::<serde_json::Value>(data) {
                // Extract text and functionCall from candidates
                if let Some(candidates) = chunk.get("candidates").and_then(|c| c.as_array()) {
                    for candidate in candidates {
                        if let Some(parts) = candidate
                            .pointer("/content/parts")
                            .and_then(|p| p.as_array())
                        {
                            for part in parts {
                                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                    full_content.push_str(text);
                                    on_chunk(text)?;
                                }
                                if let Some(fc) = part.get("functionCall") {
                                    let name = fc
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let arguments = fc.get("args").cloned().unwrap_or(
                                        serde_json::Value::Object(serde_json::Map::new()),
                                    );
                                    tool_calls.push(ToolCall {
                                        id: format!("gemini-call-{}", tool_call_idx),
                                        name,
                                        arguments,
                                        // Sibling of functionCall in the part; must be
                                        // echoed back on the next round or Gemini 2.5+
                                        // rejects the resent turn with a 400.
                                        thought_signature: part
                                            .get("thoughtSignature")
                                            .and_then(|s| s.as_str())
                                            .map(|s| s.to_string()),
                                    });
                                    tool_call_idx += 1;
                                }
                            }
                        }
                    }
                }
                // Extract usage metadata
                if let Some(usage) = chunk.get("usageMetadata") {
                    if let Some(pt) = usage.get("promptTokenCount").and_then(|v| v.as_u64()) {
                        prompt_tokens = pt as u32;
                    }
                    if let Some(ct) = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()) {
                        completion_tokens = ct as u32;
                    }
                    // cachedContentTokenCount is a SUBSET of promptTokenCount (read hits).
                    if let Some(cc) = usage
                        .get("cachedContentTokenCount")
                        .and_then(|v| v.as_u64())
                    {
                        cached_tokens = cc as u32;
                    }
                }
            }
            Ok(())
        })
        .await?;

        let stop_reason = if tool_calls.is_empty() {
            Some("stop".to_string())
        } else {
            Some("tool_use".to_string())
        };

        Ok(ChatResponse {
            content: full_content,
            role: "assistant".to_string(),
            model: model.clone(),
            tool_calls,
            usage: Usage {
                prompt_tokens,
                completion_tokens,
                model,
                cache_read_input_tokens: cached_tokens,
                cache_creation_input_tokens: 0,
            },
            stop_reason,
        })
    }

    fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        // Convert messages to Gemini format
        let mut contents = Vec::new();
        for msg in &request.messages {
            if msg.role == "system" {
                continue; // handled separately
            }
            if !msg.tool_calls.is_empty() {
                // Assistant turn → a "model" content with functionCall parts.
                let mut parts: Vec<serde_json::Value> = Vec::new();
                let text = msg.content.to_text();
                if !text.is_empty() {
                    parts.push(serde_json::json!({ "text": text }));
                }
                for tc in &msg.tool_calls {
                    let mut part = serde_json::json!({
                        "functionCall": { "name": tc.name, "args": tc.arguments }
                    });
                    // Echo the opaque thoughtSignature captured from the model's
                    // original functionCall part — Gemini 2.5+ rejects a resent
                    // tool-call turn without it (400 INVALID_ARGUMENT).
                    if let Some(sig) = &tc.thought_signature {
                        part["thoughtSignature"] = serde_json::Value::String(sig.clone());
                    }
                    parts.push(part);
                }
                contents.push(serde_json::json!({ "role": "model", "parts": parts }));
                continue;
            }
            if msg.role == "tool" {
                // Tool result → a "user" content with a functionResponse part keyed
                // by the tool name (Gemini correlates results by name).
                let name = msg.tool_name.clone().unwrap_or_default();
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": name,
                            "response": { "result": msg.content.to_text() }
                        }
                    }]
                }));
                continue;
            }
            let role = match msg.role.as_str() {
                "assistant" => "model",
                other => other,
            };
            let parts = serialize_gemini_parts(&msg.content);
            contents.push(serde_json::json!({
                "role": role,
                "parts": parts
            }));
        }

        let mut body = serde_json::json!({
            "contents": contents,
        });

        // System instruction
        let system = request.system.clone().or_else(|| {
            request
                .messages
                .iter()
                .find(|m| m.role == "system")
                .map(|m| m.content.to_text())
        });
        if let Some(sys) = system {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{"text": sys}]
            });
        }

        // Generation config
        let mut gen_config = serde_json::Map::new();
        if let Some(max_tokens) = request.max_tokens {
            gen_config.insert("maxOutputTokens".to_string(), serde_json::json!(max_tokens));
        }
        if let Some(temp) = request.temperature {
            gen_config.insert("temperature".to_string(), serde_json::json!(temp));
        }
        if !request.stop_sequences.is_empty() {
            gen_config.insert(
                "stopSequences".to_string(),
                serde_json::json!(request.stop_sequences),
            );
        }
        // Canonical reasoning_effort → Gemini thinkingConfig.thinkingBudget.
        // minimal/none → 0 (disable thinking); others scale the budget.
        if let Some(effort) = request.reasoning_effort.as_deref() {
            let budget = gemini_thinking_budget(effort);
            gen_config.insert(
                "thinkingConfig".to_string(),
                serde_json::json!({ "thinkingBudget": budget }),
            );
        }
        if !gen_config.is_empty() {
            body["generationConfig"] = serde_json::Value::Object(gen_config);
        }

        // Tools (function declarations)
        if !request.tools.is_empty() {
            let function_declarations: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|tool| {
                    serde_json::json!({
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::json!([{
                "function_declarations": function_declarations,
            }]);
        }

        body
    }

    fn parse_response(
        &self,
        resp: &serde_json::Value,
        model: &str,
    ) -> Result<ChatResponse, LlmError> {
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let mut tool_call_idx = 0usize;
        let mut finish_reason_str: Option<String> = None;

        if let Some(candidates) = resp.get("candidates").and_then(|c| c.as_array()) {
            if let Some(candidate) = candidates.first() {
                finish_reason_str = candidate
                    .get("finishReason")
                    .and_then(|f| f.as_str())
                    .map(|s| s.to_string());
                if let Some(parts) = candidate
                    .pointer("/content/parts")
                    .and_then(|p| p.as_array())
                {
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(text);
                        }
                        if let Some(fc) = part.get("functionCall") {
                            let name = fc
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();
                            let arguments = fc
                                .get("args")
                                .cloned()
                                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                            tool_calls.push(ToolCall {
                                id: format!("gemini-call-{}", tool_call_idx),
                                name,
                                arguments,
                                // Sibling of functionCall in the part; echoed back on
                                // the next round (Gemini 2.5+ 400s without it).
                                thought_signature: part
                                    .get("thoughtSignature")
                                    .and_then(|s| s.as_str())
                                    .map(|s| s.to_string()),
                            });
                            tool_call_idx += 1;
                        }
                    }
                }
            }
        }

        let mut prompt_tokens = 0u32;
        let mut completion_tokens = 0u32;
        let mut cached_tokens = 0u32;
        if let Some(usage) = resp.get("usageMetadata") {
            prompt_tokens = usage
                .get("promptTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            completion_tokens = usage
                .get("candidatesTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            // cachedContentTokenCount is a SUBSET of promptTokenCount (read hits).
            cached_tokens = usage
                .get("cachedContentTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
        }
        // Thinking tokens are reported separately and are NOT in candidatesTokenCount —
        // when a small budget is spent entirely on reasoning, the visible output is empty.
        let thoughts_tokens = resp
            .get("usageMetadata")
            .and_then(|u| u.get("thoughtsTokenCount"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        // Empty-output footgun: Gemini's default models are thinking models, so a small
        // `:max-tokens` can be spent entirely on reasoning, leaving no visible text and a
        // `MAX_TOKENS` finish — a silent empty string otherwise. Surface it instead.
        if content.is_empty()
            && tool_calls.is_empty()
            && (finish_reason_str.as_deref() == Some("MAX_TOKENS") || thoughts_tokens > 0)
        {
            return Err(LlmError::Api {
                status: 200,
                message: format!(
                    "Gemini returned an empty response (finishReason: {}; {thoughts_tokens} \
                     reasoning tokens, 0 visible output tokens). Thinking models spend part of \
                     `:max-tokens` reasoning before producing visible output — increase \
                     `:max-tokens`, or lower `:reasoning-effort`.",
                    finish_reason_str.as_deref().unwrap_or("unknown")
                ),
            });
        }

        let stop_reason = if tool_calls.is_empty() {
            Some("stop".to_string())
        } else {
            Some("tool_use".to_string())
        };

        Ok(ChatResponse {
            content,
            role: "assistant".to_string(),
            model: model.to_string(),
            tool_calls,
            usage: Usage {
                prompt_tokens,
                completion_tokens,
                model: model.to_string(),
                cache_read_input_tokens: cached_tokens,
                cache_creation_input_tokens: 0,
            },
            stop_reason,
        })
    }
}

impl LlmProvider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        sema_io::io_block_on(self.complete_async(request))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn complete_future(
        &self,
        request: ChatRequest,
    ) -> Option<crate::provider::BoxCompletionFuture<'_>> {
        Some(Box::pin(self.complete_async(request)))
    }

    fn stream_complete(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<ChatResponse, LlmError> {
        // io_block_on drives ON THIS thread: `on_chunk` may touch non-Send Sema
        // values and must never migrate to a pool worker.
        sema_io::io_block_on(self.stream_complete_async(request, on_chunk))
    }

    fn batch_complete(&self, requests: Vec<ChatRequest>) -> Vec<Result<ChatResponse, LlmError>> {
        sema_io::io_block_on(async {
            let futures: Vec<_> = requests
                .into_iter()
                .map(|req| self.complete_async(req))
                .collect();
            futures::future::join_all(futures).await
        })
    }
}

fn serialize_gemini_parts(content: &crate::types::MessageContent) -> serde_json::Value {
    match content {
        crate::types::MessageContent::Text(s) => serde_json::json!([{"text": s}]),
        crate::types::MessageContent::Blocks(blocks) => {
            let parts: Vec<serde_json::Value> = blocks
                .iter()
                .map(|b| match b {
                    crate::types::ContentBlock::Text { text } => serde_json::json!({"text": text}),
                    crate::types::ContentBlock::Image { media_type, data } => {
                        serde_json::json!({
                            "inlineData": {
                                "mimeType": media_type.as_deref().unwrap_or("application/octet-stream"),
                                "data": data
                            }
                        })
                    }
                })
                .collect();
            serde_json::Value::Array(parts)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_budget_mapping() {
        assert_eq!(gemini_thinking_budget("low"), 1024);
        assert_eq!(gemini_thinking_budget("medium"), 8192);
        assert_eq!(gemini_thinking_budget("high"), 24576);
        assert_eq!(gemini_thinking_budget("none"), 0);
        assert_eq!(gemini_thinking_budget("minimal"), 0);
    }

    #[test]
    fn build_url_omits_api_key() {
        let url = build_url(
            "https://generativelanguage.googleapis.com/v1beta",
            "gemini-3.5-flash",
            "generateContent",
        )
        .unwrap();
        assert!(
            !url.contains("key="),
            "API key must not appear in URL: {url}"
        );
        assert!(url.contains("gemini-3.5-flash"));
        assert!(url.ends_with(":generateContent"));
    }

    #[test]
    fn build_url_rejects_path_injection_in_model() {
        assert!(build_url(
            "https://x/v1beta",
            "../../v1/models/evil",
            "generateContent"
        )
        .is_err());
        assert!(build_url("https://x/v1beta", "a/b", "generateContent").is_err());
        assert!(build_url("https://x/v1beta", "bad\nmodel", "generateContent").is_err());
    }

    /// A resent assistant tool-call turn must echo the model's opaque
    /// `thoughtSignature` in its functionCall part — Gemini 2.5+ rejects the
    /// request with 400 INVALID_ARGUMENT otherwise (found live by the
    /// `make llm-stress` provider matrix; FakeProvider cannot see wire drift).
    #[test]
    fn resent_tool_call_turn_echoes_thought_signature() {
        use crate::types::{ChatMessage, ChatRequest, ToolCall};
        let provider =
            GeminiProvider::new("test-key".to_string(), Some("gemini-2.5-flash".into())).unwrap();
        let mut messages = vec![ChatMessage::new("user", "go")];
        messages.push(ChatMessage::assistant_with_tool_calls(
            String::new(),
            vec![
                ToolCall {
                    id: "gemini-call-0".into(),
                    name: "note".into(),
                    arguments: serde_json::json!({"text": "alpha"}),
                    thought_signature: Some("sig-abc".into()),
                },
                // A call without a signature (e.g. from an older cassette) must
                // serialize WITHOUT the key, not with null.
                ToolCall {
                    id: "gemini-call-1".into(),
                    name: "note".into(),
                    arguments: serde_json::json!({"text": "beta"}),
                    thought_signature: None,
                },
            ],
        ));
        let body = provider
            .build_request_body(&ChatRequest::new("gemini-2.5-flash".to_string(), messages));
        let parts = body["contents"][1]["parts"]
            .as_array()
            .expect("model parts");
        assert_eq!(
            parts[0]["thoughtSignature"], "sig-abc",
            "signature must be echoed as a sibling of functionCall"
        );
        assert!(
            parts[1].get("thoughtSignature").is_none(),
            "absent signature must serialize without the key"
        );
        assert_eq!(parts[0]["functionCall"]["name"], "note");
    }
}
