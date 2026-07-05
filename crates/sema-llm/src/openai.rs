use std::cell::RefCell;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::provider::LlmProvider;
use crate::types::{
    ChatRequest, ChatResponse, ContentBlock, EmbedRequest, EmbedResponse, LlmError, MessageContent,
    ToolCall, Usage,
};

thread_local! {
    /// Models the official OpenAI endpoint has rejected `temperature` on
    /// (gpt-5.0 / o-series reasoning models). Learned at runtime from the 400
    /// response, then omitted on subsequent requests so portable Sema code that
    /// sets `:temperature` keeps working without any change. See Phase 4 compat.
    static DROP_TEMPERATURE: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// The official OpenAI API and Azure OpenAI require the gpt-5/o-series parameter
/// conventions (`max_completion_tokens`, temperature restrictions); compatibility
/// endpoints (Ollama/OpenRouter/vLLM/…) use the legacy ones.
fn is_official_openai_url(base_url: &str) -> bool {
    base_url.contains("api.openai.com")
        || base_url.contains("openai.azure.com")
        || base_url.contains("cognitiveservices.azure.com")
}

/// Detect an OpenAI 400 that rejects a custom `temperature` (covers both
/// "'temperature' is not supported" and "does not support … Only the default").
fn mentions_unsupported_temperature(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("temperature")
        && (lower.contains("not supported")
            || lower.contains("does not support")
            || lower.contains("unsupported"))
}

pub struct OpenAiProvider {
    name: String,
    api_key: String,
    base_url: String,
    default_model: String,
    send_stream_options: bool,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(
        api_key: String,
        base_url: Option<String>,
        default_model: Option<String>,
    ) -> Result<Self, LlmError> {
        Self::named(
            "openai".to_string(),
            api_key,
            base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            default_model.unwrap_or_else(|| "gpt-5.5".to_string()),
            true,
        )
    }

    pub fn named(
        name: String,
        api_key: String,
        base_url: String,
        default_model: String,
        send_stream_options: bool,
    ) -> Result<Self, LlmError> {
        Ok(OpenAiProvider {
            name,
            api_key,
            base_url,
            default_model,
            send_stream_options,
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

    fn build_request_body(&self, request: &ChatRequest) -> OpenAiRequest {
        let model = self.resolve_model(&request.model);
        let mut messages: Vec<OpenAiMessage> = Vec::new();

        // Prepend system message if provided
        if let Some(ref system) = request.system {
            messages.push(OpenAiMessage {
                role: "system".to_string(),
                content: Some(serde_json::Value::String(system.clone())),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        for m in &request.messages {
            if !m.tool_calls.is_empty() {
                // Assistant turn that invoked tools — echo the tool_calls so the
                // following tool results can be correlated.
                let text = m.content.to_text();
                messages.push(OpenAiMessage {
                    role: "assistant".to_string(),
                    content: if text.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::String(text))
                    },
                    tool_calls: Some(
                        m.tool_calls
                            .iter()
                            .map(|tc| OpenAiToolCall {
                                id: tc.id.clone(),
                                call_type: "function".to_string(),
                                function: OpenAiFunctionCall {
                                    name: tc.name.clone(),
                                    arguments: tc.arguments.to_string(),
                                },
                            })
                            .collect(),
                    ),
                    tool_call_id: None,
                });
            } else if m.role == "tool" {
                // Tool result — must be role:"tool" with the matching tool_call_id.
                messages.push(OpenAiMessage {
                    role: "tool".to_string(),
                    content: Some(serialize_openai_content(&m.content)),
                    tool_calls: None,
                    tool_call_id: m.tool_call_id.clone(),
                });
            } else {
                messages.push(OpenAiMessage {
                    role: m.role.clone(),
                    content: Some(serialize_openai_content(&m.content)),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
        }

        let tools: Vec<OpenAiTool> = request
            .tools
            .iter()
            .map(|t| OpenAiTool {
                tool_type: "function".to_string(),
                function: OpenAiFunction {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect();

        let response_format = if request.json_mode {
            Some(ResponseFormat {
                format_type: "json_object".to_string(),
            })
        } else {
            None
        };

        // The official OpenAI API requires `max_completion_tokens` (gpt-5 /
        // o-series reject `max_tokens`); compatibility endpoints still expect the
        // legacy `max_tokens`. Pick by endpoint.
        let is_official_openai = is_official_openai_url(&self.base_url);
        let (max_tokens, max_completion_tokens) = if is_official_openai {
            (None, request.max_tokens)
        } else {
            (request.max_tokens, None)
        };
        // Omit temperature for models we've learned reject it (see DROP_TEMPERATURE).
        let temperature = if DROP_TEMPERATURE.with(|s| s.borrow().contains(&model)) {
            None
        } else {
            request.temperature
        };

        OpenAiRequest {
            model,
            messages,
            max_tokens,
            max_completion_tokens,
            reasoning_effort: request.reasoning_effort.clone(),
            temperature,
            tools,
            stop: request.stop_sequences.clone(),
            stream: None,
            stream_options: None,
            response_format,
        }
    }

    async fn complete_async(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let body = self.build_request_body(&request);

        let resp = crate::http::with_timeout(
            self.client
                .post(format!("{}/chat/completions", self.base_url)),
            request.timeout_ms,
        )
        .header("Authorization", format!("Bearer {}", self.api_key))
        .header("Content-Type", "application/json")
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

        let api_resp: OpenAiResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        let choice = api_resp
            .choices
            .first()
            .ok_or_else(|| LlmError::Parse("no choices in response".to_string()))?;

        let content = choice
            .message
            .content
            .as_ref()
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let tool_calls = choice
            .message
            .tool_calls
            .as_ref()
            .map(|tcs| {
                tcs.iter()
                    .map(|tc| ToolCall {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        arguments: serde_json::from_str(&tc.function.arguments)
                            .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
                        thought_signature: None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let usage = api_resp
            .usage
            .map(|u| {
                let cached = u.prompt_tokens_details.as_ref();
                Usage {
                    prompt_tokens: u.prompt_tokens,
                    completion_tokens: u.completion_tokens,
                    model: api_resp.model.clone(),
                    // cached_tokens is a SUBSET of prompt_tokens (read hits).
                    cache_read_input_tokens: cached.map(|d| d.cached_tokens).unwrap_or(0),
                    cache_creation_input_tokens: cached.map(|d| d.cache_write_tokens).unwrap_or(0),
                }
            })
            .unwrap_or_default();

        Ok(ChatResponse {
            content,
            role: "assistant".to_string(),
            model: api_resp.model,
            tool_calls,
            usage,
            stop_reason: choice.finish_reason.clone(),
        })
    }

    async fn stream_complete_async(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<ChatResponse, LlmError> {
        let mut body = self.build_request_body(&request);
        body.stream = Some(true);
        if self.send_stream_options {
            body.stream_options = Some(StreamOptions {
                include_usage: true,
            });
        }
        let model_name = body.model.clone();

        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
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
        let mut cache_read_input_tokens = 0u32;
        let mut cache_creation_input_tokens = 0u32;
        let mut finish_reason = None;
        // Streamed tool calls arrive as index-keyed fragments: the first delta for an
        // index carries `id` + `function.name`, later deltas append `function.arguments`
        // chunks. Accumulate (id, name, args) per index, then assemble at the end.
        let mut tool_acc: Vec<(String, String, String)> = Vec::new();

        crate::sse::parse_sse_stream(resp, |data| {
            if let Ok(chunk) = serde_json::from_str::<serde_json::Value>(data) {
                // Extract usage from final chunk
                if let Some(usage) = chunk.get("usage") {
                    if !usage.is_null() {
                        prompt_tokens = usage
                            .get("prompt_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        completion_tokens = usage
                            .get("completion_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        // cached_tokens is a SUBSET of prompt_tokens (read hits).
                        if let Some(details) = usage.get("prompt_tokens_details") {
                            cache_read_input_tokens = details
                                .get("cached_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0)
                                as u32;
                            cache_creation_input_tokens = details
                                .get("cache_write_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0)
                                as u32;
                        }
                    }
                }
                // Extract content delta
                if let Some(choices) = chunk.get("choices").and_then(|c| c.as_array()) {
                    for choice in choices {
                        if let Some(delta) = choice.get("delta") {
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                full_content.push_str(content);
                                on_chunk(content)?;
                            }
                            if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                                for tc in tcs {
                                    let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0)
                                        as usize;
                                    while tool_acc.len() <= idx {
                                        tool_acc.push((
                                            String::new(),
                                            String::new(),
                                            String::new(),
                                        ));
                                    }
                                    let entry = &mut tool_acc[idx];
                                    if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                                        if !id.is_empty() {
                                            entry.0 = id.to_string();
                                        }
                                    }
                                    if let Some(f) = tc.get("function") {
                                        if let Some(name) = f.get("name").and_then(|v| v.as_str()) {
                                            entry.1.push_str(name);
                                        }
                                        if let Some(args) =
                                            f.get("arguments").and_then(|v| v.as_str())
                                        {
                                            entry.2.push_str(args);
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(fr) = choice.get("finish_reason") {
                            if !fr.is_null() {
                                finish_reason = fr.as_str().map(|s| s.to_string());
                            }
                        }
                    }
                }
            }
            Ok(())
        })
        .await?;

        let tool_calls: Vec<ToolCall> = tool_acc
            .into_iter()
            .filter(|(_, name, _)| !name.is_empty())
            .map(|(id, name, args)| ToolCall {
                id,
                name,
                arguments: serde_json::from_str(&args)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
                thought_signature: None,
            })
            .collect();

        Ok(ChatResponse {
            content: full_content,
            role: "assistant".to_string(),
            model: model_name.clone(),
            tool_calls,
            usage: Usage {
                prompt_tokens,
                completion_tokens,
                model: model_name,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            },
            stop_reason: finish_reason,
        })
    }

    async fn embed_async(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let model = request
            .model
            .unwrap_or_else(|| "text-embedding-3-small".to_string());

        let body = serde_json::json!({
            "input": request.texts,
            "model": model,
        });

        let resp = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let status = resp.status().as_u16();
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

        let resp_model = api_resp
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or(&model)
            .to_string();

        let embeddings = api_resp
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| LlmError::Parse("missing data in embedding response".to_string()))?
            .iter()
            .filter_map(|item| {
                item.get("embedding")
                    .and_then(|e| e.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect::<Vec<f64>>())
            })
            .collect::<Vec<Vec<f64>>>();

        let usage = api_resp
            .get("usage")
            .map(|u| Usage {
                prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                completion_tokens: 0,
                model: resp_model.clone(),
                ..Default::default()
            })
            .unwrap_or_default();

        Ok(EmbedResponse {
            embeddings,
            model: resp_model,
            usage,
        })
    }
}

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    /// Legacy token cap — accepted by OpenAI-compatible endpoints (Ollama,
    /// OpenRouter, vLLM, …) and older OpenAI models.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    /// The current OpenAI cap — required by gpt-5 / o-series models, which reject
    /// `max_tokens`. Sent only when targeting the official OpenAI endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    /// gpt-5 / o-series reasoning control (minimal|low|medium|high|none|xhigh).
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiTool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Serialize, Deserialize)]
struct OpenAiMessage {
    role: String,
    content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
    /// Set on `role: "tool"` messages to correlate the result with the call.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct OpenAiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAiFunction,
}

#[derive(Serialize, Deserialize)]
struct OpenAiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OpenAiFunctionCall,
}

#[derive(Serialize, Deserialize)]
struct OpenAiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    model: String,
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    /// Cached prompt tokens (a SUBSET of prompt_tokens). Present on OpenAI and most
    /// OpenAI-compatible providers (Groq/Together/Fireworks/vLLM/OpenRouter).
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
}

#[derive(Deserialize)]
struct OpenAiPromptTokensDetails {
    #[serde(default)]
    cached_tokens: u32,
    /// OpenRouter surfaces cache-write tokens for models that price them (e.g.
    /// Anthropic via OpenRouter). Absent on native OpenAI.
    #[serde(default)]
    cache_write_tokens: u32,
}

impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        // GUARD: two io_block_on calls in this method, STRICTLY SEQUENTIAL and
        // NEVER NESTED — the first fully returns before the retry's begins.
        // Nesting a block_on inside a block_on'd future would panic (see the
        // sema-io threading contract / tokio pin tests).
        let result = sema_io::io_block_on(self.complete_async(request.clone()));
        // Compat backstop: gpt-5.0 / o-series reject a custom `temperature` with a
        // 400. Learn it per-model, drop temperature, and retry once — so portable
        // Sema code that sets :temperature still works on those models.
        if request.temperature.is_some() && is_official_openai_url(&self.base_url) {
            if let Err(LlmError::Api {
                status: 400,
                message,
            }) = &result
            {
                if mentions_unsupported_temperature(message) {
                    let model = self.resolve_model(&request.model);
                    DROP_TEMPERATURE.with(|s| s.borrow_mut().insert(model));
                    return sema_io::io_block_on(self.complete_async(request));
                }
            }
        }
        result
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn complete_future(
        &self,
        request: ChatRequest,
    ) -> Option<crate::provider::BoxCompletionFuture<'_>> {
        Some(Box::pin(async move {
            let result = self.complete_async(request.clone()).await;
            // Same DROP_TEMPERATURE compat backstop as `complete()` (see there),
            // with one async-path nuance: the future may resume on a DIFFERENT
            // pool worker after the first attempt, so the retry must not depend
            // on the thread-local learn — strip `temperature` from the retried
            // request itself (wire-identical to the TLS drop in
            // `build_request_body`). The TLS insert still happens so later calls
            // that land on this worker skip the doomed first request.
            if request.temperature.is_some() && is_official_openai_url(&self.base_url) {
                if let Err(LlmError::Api {
                    status: 400,
                    message,
                }) = &result
                {
                    if mentions_unsupported_temperature(message) {
                        let model = self.resolve_model(&request.model);
                        DROP_TEMPERATURE.with(|s| s.borrow_mut().insert(model));
                        let mut retry = request;
                        retry.temperature = None;
                        return self.complete_async(retry).await;
                    }
                }
            }
            result
        }))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn embed_future(&self, request: EmbedRequest) -> Option<crate::provider::BoxEmbedFuture<'_>> {
        Some(Box::pin(self.embed_async(request)))
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

    fn embed(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        sema_io::io_block_on(self.embed_async(request))
    }
}

fn serialize_openai_content(content: &MessageContent) -> serde_json::Value {
    match content {
        MessageContent::Text(s) => serde_json::Value::String(s.clone()),
        MessageContent::Blocks(blocks) => {
            let arr: Vec<serde_json::Value> = blocks
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => serde_json::json!({
                        "type": "text",
                        "text": text
                    }),
                    ContentBlock::Image { media_type, data } => {
                        let mt = media_type.as_deref().unwrap_or("application/octet-stream");
                        serde_json::json!({
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:{mt};base64,{data}")
                            }
                        })
                    }
                })
                .collect();
            serde_json::Value::Array(arr)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, ChatRequest, ToolCall};

    fn req_with_max() -> ChatRequest {
        let mut r = ChatRequest::new("gpt-5.4-mini".into(), vec![ChatMessage::new("user", "hi")]);
        r.max_tokens = Some(100);
        r
    }

    #[test]
    fn official_endpoint_detection_covers_azure() {
        assert!(is_official_openai_url("https://api.openai.com/v1"));
        assert!(is_official_openai_url(
            "https://my-resource.openai.azure.com"
        ));
        assert!(is_official_openai_url(
            "https://my-resource.cognitiveservices.azure.com"
        ));
        assert!(!is_official_openai_url("http://localhost:11434/v1")); // Ollama
        assert!(!is_official_openai_url("https://openrouter.ai/api/v1"));
    }

    #[test]
    fn detects_unsupported_temperature_400() {
        assert!(mentions_unsupported_temperature(
            "Unsupported parameter: 'temperature' is not supported with this model."
        ));
        assert!(mentions_unsupported_temperature(
            "Unsupported value: 'temperature' does not support 0 with this model. Only the default (1) value is supported."
        ));
        assert!(!mentions_unsupported_temperature(
            "Unsupported parameter: 'max_tokens' is not supported with this model."
        ));
    }

    #[test]
    fn memoized_model_drops_temperature() {
        let p = OpenAiProvider::new("k".into(), None, None).unwrap();
        let mut r = ChatRequest::new("gpt-5.0".into(), vec![ChatMessage::new("user", "hi")]);
        r.temperature = Some(0.2);
        // Before learning: temperature is sent.
        assert_eq!(p.build_request_body(&r).temperature, Some(0.2));
        // After learning the model rejects it: omitted.
        DROP_TEMPERATURE.with(|s| s.borrow_mut().insert("gpt-5.0".to_string()));
        assert_eq!(p.build_request_body(&r).temperature, None);
        DROP_TEMPERATURE.with(|s| s.borrow_mut().clear());
    }

    #[test]
    fn forwards_reasoning_effort() {
        let p = OpenAiProvider::new("k".into(), None, None).unwrap();
        let mut r = ChatRequest::new("gpt-5.4-mini".into(), vec![ChatMessage::new("user", "hi")]);
        r.reasoning_effort = Some("high".into());
        assert_eq!(
            p.build_request_body(&r).reasoning_effort.as_deref(),
            Some("high")
        );
    }

    #[test]
    fn official_openai_uses_max_completion_tokens() {
        // gpt-5 / o-series reject `max_tokens`; the official endpoint must send
        // `max_completion_tokens` instead.
        let p = OpenAiProvider::new("k".into(), None, None).unwrap();
        let body = p.build_request_body(&req_with_max());
        assert_eq!(body.max_completion_tokens, Some(100));
        assert_eq!(body.max_tokens, None);
    }

    #[test]
    fn compat_endpoint_uses_legacy_max_tokens() {
        // OpenAI-compatible endpoints (Ollama/OpenRouter/vLLM) expect `max_tokens`.
        let p =
            OpenAiProvider::new("k".into(), Some("http://localhost:1234/v1".into()), None).unwrap();
        let body = p.build_request_body(&req_with_max());
        assert_eq!(body.max_tokens, Some(100));
        assert_eq!(body.max_completion_tokens, None);
    }

    #[test]
    fn tool_results_serialize_with_correlation() {
        // Phase 2: assistant turn carries tool_calls; the result is a role:"tool"
        // message keyed by tool_call_id.
        let p = OpenAiProvider::new("k".into(), None, None).unwrap();
        let mut r = ChatRequest::new(
            "gpt-5.4-mini".into(),
            vec![
                ChatMessage::new("user", "weather?"),
                ChatMessage::assistant_with_tool_calls(
                    "",
                    vec![ToolCall {
                        id: "call_1".into(),
                        name: "get_weather".into(),
                        arguments: serde_json::json!({"city": "Oslo"}),
                        thought_signature: None,
                    }],
                ),
                ChatMessage::tool_result("call_1", "get_weather", "sunny"),
            ],
        );
        r.max_tokens = Some(100);
        let body = p.build_request_body(&r);

        let assistant = body
            .messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant message present");
        assert!(
            assistant.tool_calls.as_ref().is_some_and(|t| !t.is_empty()),
            "assistant message must echo tool_calls"
        );

        let tool = body
            .messages
            .iter()
            .find(|m| m.role == "tool")
            .expect("tool result message present");
        assert_eq!(tool.tool_call_id.as_deref(), Some("call_1"));
    }
}
