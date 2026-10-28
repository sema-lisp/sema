use crate::provider::LlmProvider;
use crate::types::{ChatRequest, ChatResponse, LlmError, ToolCall, ToolSchema, Usage};

/// Convert our `ToolSchema` list into Ollama's (OpenAI-compatible) tools JSON.
fn build_tools_json(tools: &[ToolSchema]) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();
    serde_json::Value::Array(arr)
}

/// Parse tool calls from an Ollama response JSON value.
/// Ollama returns `message.tool_calls` as an array of `{ function: { name, arguments } }`.
/// We generate synthetic IDs since Ollama doesn't provide them.
fn parse_tool_calls(message: &serde_json::Value) -> Vec<ToolCall> {
    let Some(arr) = message.get("tool_calls").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .enumerate()
        .filter_map(|(i, tc)| {
            let func = tc.get("function")?;
            let name = func.get("name")?.as_str()?.to_string();
            let arguments = func
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default()));
            Some(ToolCall {
                id: format!("ollama-call-{i}"),
                name,
                arguments,
                thought_signature: None,
            })
        })
        .collect()
}

/// Build the Ollama `messages` array, handling assistant tool_calls and
/// correlated tool results (Ollama's chat API is OpenAI-shaped).
fn build_ollama_messages(request: &ChatRequest) -> Vec<serde_json::Value> {
    let mut messages = Vec::new();
    if let Some(ref system) = request.system {
        messages.push(serde_json::json!({ "role": "system", "content": system }));
    }
    for msg in &request.messages {
        match msg.kind() {
            crate::types::MessageKind::AssistantWithToolCalls(content, tcs) => {
                // Assistant turn that invoked tools (Ollama takes arguments as an object).
                let tcs: Vec<serde_json::Value> = tcs
                    .iter()
                    .map(|tc| serde_json::json!({ "function": { "name": tc.name, "arguments": tc.arguments } }))
                    .collect();
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": content.to_text(),
                    "tool_calls": tcs,
                }));
            }
            crate::types::MessageKind::ToolResult {
                id: _,
                name,
                content,
            } => {
                let mut m = serde_json::json!({ "role": "tool", "content": content.to_text() });
                if let Some(name) = name {
                    m["tool_name"] = serde_json::json!(name);
                }
                messages.push(m);
            }
            crate::types::MessageKind::Other(role, content) => {
                let mut m = serde_json::json!({ "role": role, "content": content.to_text() });
                if let crate::types::MessageContent::Blocks(blocks) = content {
                    let images: Vec<&str> = blocks
                        .iter()
                        .filter_map(|b| {
                            if let crate::types::ContentBlock::Image { data, .. } = b {
                                Some(data.as_str())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !images.is_empty() {
                        m["images"] = serde_json::json!(images);
                    }
                }
                messages.push(m);
            }
        }
    }
    messages
}

pub struct OllamaProvider {
    host: String,
    default_model: String,
    embedding_model: String,
    embedding_options: crate::types::EmbedOptions,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(host: Option<String>, default_model: Option<String>) -> Result<Self, LlmError> {
        let client = crate::http::create_client(None)?;
        Ok(OllamaProvider {
            host: normalize_host(&host.unwrap_or_else(|| {
                std::env::var("OLLAMA_HOST")
                    .unwrap_or_else(|_| "http://localhost:11434".to_string())
            }))?,
            embedding_model: "embeddinggemma".to_string(),
            embedding_options: Default::default(),
            default_model: default_model.unwrap_or_else(|| "gemma4".to_string()),
            client,
        })
    }

    /// Select an embedding model without changing the chat default.
    pub fn with_embedding_model(mut self, model: String) -> Self {
        self.embedding_model = model;
        self
    }

    /// Set defaults for individual embedding request options.
    pub fn with_embedding_options(mut self, options: crate::types::EmbedOptions) -> Self {
        self.embedding_options = options;
        self
    }

    async fn embed_async(
        &self,
        request: crate::types::EmbedRequest,
    ) -> Result<crate::types::EmbedResponse, LlmError> {
        let model = request
            .model
            .unwrap_or_else(|| self.embedding_model.clone());
        let options = request.options.with_defaults(&self.embedding_options);
        let mut body = serde_json::json!({"model": model, "input": request.texts});
        if let Some(n) = options.dimensions {
            body["dimensions"] = serde_json::json!(n);
        }
        if let Some(b) = options.truncate {
            body["truncate"] = serde_json::json!(b);
        }
        if let Some(v) = options.keep_alive {
            body["keep_alive"] = v;
        }
        let response = crate::http::with_timeout(
            self.client.post(format!("{}/api/embed", self.host)),
            options.timeout_ms,
        )
        .json(&body)
        .send()
        .await
        .map_err(|e| LlmError::Http(e.to_string()))?;
        let response = crate::http::check_status(response).await?;
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;
        let embeddings =
            serde_json::from_value(body.get("embeddings").cloned().ok_or_else(|| {
                LlmError::Parse("missing embeddings in Ollama response".to_string())
            })?)
            .map_err(|e| LlmError::Parse(e.to_string()))?;
        Ok(crate::types::EmbedResponse {
            embeddings,
            model: model.clone(),
            usage: Usage {
                model,
                prompt_tokens: body
                    .get("prompt_eval_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                ..Default::default()
            },
        })
    }

    async fn complete_async(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let model = crate::provider::resolve_model(&request.model, &self.default_model);
        let url = format!("{}/api/chat", self.host);

        let body = chat_body(&request, &model, false);

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let resp = crate::http::check_status(resp).await?;

        let api_resp: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        let content = api_resp
            .pointer("/message/content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let tool_calls = api_resp
            .get("message")
            .map(parse_tool_calls)
            .unwrap_or_default();

        let stop_reason = if tool_calls.is_empty() {
            "stop"
        } else {
            "tool_use"
        };

        let mut usage = Usage {
            model: model.clone(),
            ..Default::default()
        };
        usage.merge_json(&api_resp, &USAGE_FIELDS);

        Ok(ChatResponse {
            content,
            role: "assistant".to_string(),
            model,
            tool_calls,
            usage,
            stop_reason: Some(stop_reason.to_string()),
        })
    }

    async fn stream_complete_async(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<ChatResponse, LlmError> {
        let model = crate::provider::resolve_model(&request.model, &self.default_model);
        let url = format!("{}/api/chat", self.host);

        let body = chat_body(&request, &model, true);

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        let resp = crate::http::check_status(resp).await?;

        let mut full_content = String::new();
        let mut usage = Usage {
            model: model.clone(),
            ..Default::default()
        };
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        crate::ndjson::parse_ndjson_stream(resp, |json| {
            // Extract content delta
            if let Some(content) = json.pointer("/message/content").and_then(|v| v.as_str()) {
                if !content.is_empty() {
                    full_content.push_str(content);
                    on_chunk(content)?;
                }
            }

            // Check if done — final chunk has usage info and tool calls
            if let Some(true) = json.get("done").and_then(|v| v.as_bool()) {
                usage.merge_json(json, &USAGE_FIELDS);

                // Tool calls appear in the final message
                if let Some(msg) = json.get("message") {
                    tool_calls = parse_tool_calls(msg);
                }
            }
            Ok(())
        })
        .await?;

        let stop_reason = if tool_calls.is_empty() {
            "stop"
        } else {
            "tool_use"
        };

        Ok(ChatResponse {
            content: full_content,
            role: "assistant".to_string(),
            model,
            tool_calls,
            usage,
            stop_reason: Some(stop_reason.to_string()),
        })
    }
}

/// Token counts sit at the top level of the final `/api/chat` object; Ollama
/// reports no prompt-cache counters.
const USAGE_FIELDS: crate::types::UsageFields = crate::types::UsageFields {
    prompt: "/prompt_eval_count",
    completion: "/eval_count",
    cache_read: None,
    cache_write: None,
};

impl LlmProvider for OllamaProvider {
    fn embedding_endpoint(&self) -> Option<&str> {
        Some(&self.host)
    }

    fn embedding_options(&self) -> crate::types::EmbedOptions {
        self.embedding_options.clone()
    }

    fn default_embedding_model(&self) -> &str {
        &self.embedding_model
    }

    fn embed(
        &self,
        request: crate::types::EmbedRequest,
    ) -> Result<crate::types::EmbedResponse, LlmError> {
        sema_io::io_block_on(self.embed_async(request))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn embed_future(
        &self,
        request: crate::types::EmbedRequest,
    ) -> Option<crate::provider::BoxEmbedFuture<'_>> {
        Some(Box::pin(self.embed_async(request)))
    }

    fn name(&self) -> &str {
        "ollama"
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
}

/// The `/api/chat` request body for `request`, in Ollama's wire format.
/// Shared by the blocking and streaming paths so the two cannot drift.
fn chat_body(request: &ChatRequest, model: &str, stream: bool) -> serde_json::Value {
    let messages = build_ollama_messages(request);

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": stream,
    });

    // Add tools if provided
    if !request.tools.is_empty() {
        body["tools"] = build_tools_json(&request.tools);
    }

    // Options
    let mut options = serde_json::Map::new();
    if let Some(max_tokens) = request.max_tokens {
        options.insert("num_predict".to_string(), serde_json::json!(max_tokens));
    }
    if let Some(temp) = request.temperature {
        options.insert("temperature".to_string(), serde_json::json!(temp));
    }
    // Canonical `ChatRequest` fields Ollama does support, and was silently
    // dropping. Per the one-canonical-request rule these must be translated
    // here rather than being no-ops the caller cannot see.
    if !request.stop_sequences.is_empty() {
        options.insert(
            "stop".to_string(),
            serde_json::json!(request.stop_sequences),
        );
    }
    if !options.is_empty() {
        body["options"] = serde_json::Value::Object(options);
    }
    if request.json_mode {
        body["format"] = serde_json::json!("json");
    }
    if let Some(ref effort) = request.reasoning_effort {
        // Ollama exposes reasoning as a boolean switch, so map the tiers:
        // "none" turns thinking off, anything else turns it on.
        body["think"] = serde_json::json!(effort != "none");
    }
    body
}

/// Accept the host:port form used by OLLAMA_HOST as well as proxy URLs.
fn normalize_host(host: &str) -> Result<String, LlmError> {
    let host = host.trim();
    if host.is_empty() {
        return Ok("http://localhost:11434".to_string());
    }
    let bare = !host.contains("://");
    let value = if bare {
        format!("http://{host}")
    } else {
        host.to_string()
    };
    let mut url = reqwest::Url::parse(&value)
        .map_err(|e| LlmError::Config(format!("invalid Ollama host: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(LlmError::Config(
            "Ollama host must be an HTTP(S) URL".to_string(),
        ));
    }
    if bare && url.port().is_none() {
        url.set_port(Some(11434))
            .map_err(|_| LlmError::Config("invalid Ollama port".to_string()))?;
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}
