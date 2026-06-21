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
    runtime: crate::http::BlockingRuntime,
}

impl GeminiProvider {
    pub fn new(api_key: String, default_model: Option<String>) -> Result<Self, LlmError> {
        let runtime = crate::http::create_runtime()?;
        Ok(GeminiProvider {
            api_key,
            base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
            default_model: default_model.unwrap_or_else(|| "gemini-3.5-flash".to_string()),
            client: crate::http::create_client(None)?,
            runtime,
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
                    parts.push(serde_json::json!({
                        "functionCall": { "name": tc.name, "args": tc.arguments }
                    }));
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

        if let Some(candidates) = resp.get("candidates").and_then(|c| c.as_array()) {
            if let Some(candidate) = candidates.first() {
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
                            });
                            tool_call_idx += 1;
                        }
                    }
                }
            }
        }

        let mut prompt_tokens = 0u32;
        let mut completion_tokens = 0u32;
        if let Some(usage) = resp.get("usageMetadata") {
            prompt_tokens = usage
                .get("promptTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            completion_tokens = usage
                .get("candidatesTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
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
        self.runtime.block_on(self.complete_async(request))
    }

    fn stream_complete(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<ChatResponse, LlmError> {
        self.runtime
            .block_on(self.stream_complete_async(request, on_chunk))
    }

    fn batch_complete(&self, requests: Vec<ChatRequest>) -> Vec<Result<ChatResponse, LlmError>> {
        self.runtime.block_on(async {
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
}
