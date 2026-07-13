use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

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

    /// Models that reject a non-`none` `reasoning_effort` when function tools are
    /// present on `/chat/completions` (gpt-5.6 and newer: "Function tools with
    /// reasoning_effort are not supported … use /v1/responses or set
    /// reasoning_effort to 'none'"). Learned at runtime from that 400. Kept
    /// REACTIVE rather than proactive-per-family because some reasoning models
    /// (e.g. gpt-5.1) DO accept tools+effort — forcing `none` for every reasoning
    /// model would needlessly disable their reasoning.
    static FORCE_EFFORT_NONE: RefCell<HashSet<String>> = RefCell::new(HashSet::new());

    /// Per-model set of `reasoning_effort` values the model actually accepts,
    /// learned by READING the "Supported values are: …" list out of an
    /// unsupported-value 400 (e.g. gpt-5.6 accepts none/low/medium/high/xhigh but
    /// not `minimal`/`max`). A later request with a rejected effort is clamped to
    /// the nearest accepted value instead of hard-failing the turn.
    static EFFORT_SUPPORTED: RefCell<HashMap<String, Vec<String>>> =
        RefCell::new(HashMap::new());
}

/// Canonical reasoning-effort tiers, low → high, for nearest-match clamping.
const EFFORT_ORDER: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The official OpenAI API and Azure OpenAI require the gpt-5/o-series parameter
/// conventions (`max_completion_tokens`, temperature restrictions); compatibility
/// endpoints (Ollama/OpenRouter/vLLM/…) use the legacy ones.
fn is_official_openai_url(base_url: &str) -> bool {
    base_url.contains("api.openai.com")
        || base_url.contains("openai.azure.com")
        || base_url.contains("cognitiveservices.azure.com")
}

/// Heuristic: is MODEL an OpenAI reasoning model (gpt-5 series or o-series)?
/// Reasoning models reject a custom `temperature` (only the default is accepted)
/// — a stable, documented family rule we apply PROACTIVELY so a portable
/// `:temperature` becomes a no-op there without paying a doomed first request.
/// The `*-chat*` aliases (e.g. gpt-5-chat-latest) are non-reasoning. Anything the
/// heuristic misses is still caught by the reactive DROP_TEMPERATURE net.
fn is_reasoning_model(model: &str) -> bool {
    (model.starts_with("gpt-5") && !model.contains("chat"))
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
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

/// Detect the OpenAI 400 that rejects `reasoning_effort` alongside function
/// tools on `/chat/completions` — e.g. "Function tools with reasoning_effort
/// are not supported for gpt-5.6-luna in /v1/chat/completions. To use function
/// tools, use /v1/responses or set reasoning_effort to 'none'."
fn mentions_reasoning_effort_tools_conflict(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("reasoning_effort")
        && lower.contains("tool")
        && (lower.contains("not supported") || lower.contains("/v1/responses"))
}

/// Detect the OpenAI 400 that rejects a `reasoning_effort` VALUE (not the
/// tools interaction) — e.g. "Unsupported value: 'reasoning_effort' does not
/// support 'minimal' with this model. Supported values are: 'none', 'low', …".
fn mentions_unsupported_effort_value(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("reasoning_effort")
        && lower.contains("does not support")
        && !lower.contains("tool")
}

/// Pull the accepted effort values out of an unsupported-value 400 by matching
/// the canonical tiers that appear (quoted) in the message. Returns None if none
/// are found (so the caller leaves the request unchanged).
fn parse_unsupported_effort(body: &str) -> Option<Vec<String>> {
    let lower = body.to_lowercase();
    let found: Vec<String> = EFFORT_ORDER
        .iter()
        .filter(|tier| lower.contains(&format!("'{tier}'")))
        // The rejected value itself is also quoted; keep only those listed after
        // "supported values", i.e. drop the one the message says is unsupported.
        .filter(|tier| !lower.contains(&format!("support '{tier}'")))
        .map(|t| t.to_string())
        .collect();
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

/// Clamp a requested effort to the nearest value in SUPPORTED (by canonical
/// tier distance; ties prefer the higher tier). Returns the request unchanged if
/// it is already supported or SUPPORTED is empty.
fn clamp_effort(requested: &str, supported: &[String]) -> String {
    if supported.iter().any(|s| s == requested) || supported.is_empty() {
        return requested.to_string();
    }
    let idx = |e: &str| EFFORT_ORDER.iter().position(|t| *t == e);
    let Some(req_idx) = idx(requested) else {
        return requested.to_string();
    };
    supported
        .iter()
        .filter_map(|s| idx(s).map(|i| (s, i)))
        .min_by_key(|(_, i)| (req_idx.abs_diff(*i), usize::MAX - *i))
        .map(|(s, _)| s.clone())
        .unwrap_or_else(|| requested.to_string())
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
        // Reasoning models (gpt-5 series, o-series) reject a custom `temperature`
        // — only the default is accepted. Drop it PROACTIVELY for those families,
        // and REACTIVELY for any model learned at runtime (DROP_TEMPERATURE), so a
        // portable `:temperature` stays a harmless no-op there.
        let reasoning = is_official_openai && is_reasoning_model(&model);
        let temperature = if reasoning || DROP_TEMPERATURE.with(|s| s.borrow().contains(&model)) {
            None
        } else {
            request.temperature
        };
        // reasoning_effort. On `/chat/completions` a model we've learned rejects
        // effort+tools must send exactly `none` WHEN TOOLS ARE PRESENT — any other
        // value, including omitting it, 400s. (Gated on tools: a tool-free call to
        // the same model must keep the caller's effort.) Otherwise honor the
        // requested effort, clamped to the model's learned accepted set if known.
        let has_tools = !tools.is_empty();
        let reasoning_effort = if is_official_openai
            && has_tools
            && FORCE_EFFORT_NONE.with(|s| s.borrow().contains(&model))
        {
            Some("none".to_string())
        } else {
            request.reasoning_effort.clone().map(|eff| {
                EFFORT_SUPPORTED.with(|s| match s.borrow().get(&model) {
                    Some(sup) => clamp_effort(&eff, sup),
                    None => eff,
                })
            })
        };

        OpenAiRequest {
            model,
            messages,
            max_tokens,
            max_completion_tokens,
            reasoning_effort,
            temperature,
            tools,
            stop: request.stop_sequences.clone(),
            stream: None,
            stream_options: None,
            response_format,
        }
    }

    /// Inspect a 400 for a known OpenAI compat quirk on THIS request; if found,
    /// learn it (per-model) and return a corrected request to retry, else None.
    /// Corrections compose — a caller retry loop applies this repeatedly so a
    /// request tripping several quirks at once (temperature AND effort) recovers.
    /// Only official OpenAI raises these; compat endpoints return None.
    fn correct_for_400(&self, request: &ChatRequest, message: &str) -> Option<ChatRequest> {
        if !is_official_openai_url(&self.base_url) {
            return None;
        }
        let model = self.resolve_model(&request.model);

        // Custom temperature rejected (reasoning models).
        if request.temperature.is_some() && mentions_unsupported_temperature(message) {
            DROP_TEMPERATURE.with(|s| s.borrow_mut().insert(model));
            let mut retry = request.clone();
            retry.temperature = None;
            return Some(retry);
        }

        // Function tools + a non-`none` effort on /chat/completions. Pin to
        // `none` (also covers the omitted-effort case, which 400s the same way).
        if mentions_reasoning_effort_tools_conflict(message)
            && request.reasoning_effort.as_deref() != Some("none")
        {
            FORCE_EFFORT_NONE.with(|s| s.borrow_mut().insert(model));
            let mut retry = request.clone();
            retry.reasoning_effort = Some("none".to_string());
            return Some(retry);
        }

        // Unsupported reasoning_effort VALUE — read the accepted set from the
        // error and clamp to the nearest, so a portable effort keeps working.
        if let Some(eff) = request.reasoning_effort.clone() {
            if mentions_unsupported_effort_value(message) {
                if let Some(supported) = parse_unsupported_effort(message) {
                    let clamped = clamp_effort(&eff, &supported);
                    if clamped != eff {
                        EFFORT_SUPPORTED.with(|s| s.borrow_mut().insert(model, supported));
                        let mut retry = request.clone();
                        retry.reasoning_effort = Some(clamped);
                        return Some(retry);
                    }
                }
            }
        }

        None
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
        // GUARD: the io_block_on calls in this method are STRICTLY SEQUENTIAL and
        // NEVER NESTED — each fully returns before the next begins. Nesting a
        // block_on inside a block_on'd future would panic (see the sema-io
        // threading contract / tokio pin tests).
        //
        // Compat self-heal: on a 400 naming a known quirk (temperature rejected,
        // effort+tools conflict, unsupported effort value), learn it and retry
        // with a corrected request. Bounded so several quirks can chain without
        // ever looping on a persistent 400.
        let mut request = request;
        let mut result = sema_io::io_block_on(self.complete_async(request.clone()));
        for _ in 0..EFFORT_ORDER.len() {
            let Err(LlmError::Api {
                status: 400,
                message,
            }) = &result
            else {
                break;
            };
            let Some(corrected) = self.correct_for_400(&request, message) else {
                break;
            };
            request = corrected;
            result = sema_io::io_block_on(self.complete_async(request.clone()));
        }
        result
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn complete_future(
        &self,
        request: ChatRequest,
    ) -> Option<crate::provider::BoxCompletionFuture<'_>> {
        Some(Box::pin(async move {
            // Same chained compat self-heal as `complete()`. Async-path nuance:
            // the future may resume on a different pool worker, so the retried
            // request carries its own correction (correct_for_400 rewrites the
            // request, not just the thread-local) — the TLS learn is only an
            // optimization for later calls that land on this worker.
            let mut request = request;
            let mut result = self.complete_async(request.clone()).await;
            for _ in 0..EFFORT_ORDER.len() {
                let Err(LlmError::Api {
                    status: 400,
                    message,
                }) = &result
                else {
                    break;
                };
                let Some(corrected) = self.correct_for_400(&request, message) else {
                    break;
                };
                request = corrected;
                result = self.complete_async(request.clone()).await;
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
        // values and must never migrate to a pool worker. The io_block_on calls
        // below are STRICTLY SEQUENTIAL and NEVER NESTED (see `complete`).
        //
        // Same chained compat self-heal as `complete()`. A 400 lands before any
        // chunk is emitted (status is checked before the SSE parse), so retrying
        // can't double up on `on_chunk`.
        let mut request = request;
        let mut result =
            sema_io::io_block_on(self.stream_complete_async(request.clone(), on_chunk));
        for _ in 0..EFFORT_ORDER.len() {
            let Err(LlmError::Api {
                status: 400,
                message,
            }) = &result
            else {
                break;
            };
            let Some(corrected) = self.correct_for_400(&request, message) else {
                break;
            };
            request = corrected;
            result = sema_io::io_block_on(self.stream_complete_async(request.clone(), on_chunk));
        }
        result
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
    use crate::types::{ChatMessage, ChatRequest, ToolCall, ToolSchema};

    fn req_with_max() -> ChatRequest {
        let mut r = ChatRequest::new("gpt-5.4-mini".into(), vec![ChatMessage::new("user", "hi")]);
        r.max_tokens = Some(100);
        r
    }

    fn tool() -> ToolSchema {
        ToolSchema {
            name: "get_weather".into(),
            description: "weather".into(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    /// Clear the runtime-learned quirk caches so a test starts clean regardless of
    /// what ran before it on a reused test thread (thread_locals persist per-thread).
    fn reset_learned() {
        DROP_TEMPERATURE.with(|s| s.borrow_mut().clear());
        FORCE_EFFORT_NONE.with(|s| s.borrow_mut().clear());
        EFFORT_SUPPORTED.with(|s| s.borrow_mut().clear());
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
    fn temperature_dropped_for_reasoning_models() {
        reset_learned();
        let p = OpenAiProvider::new("k".into(), None, None).unwrap();
        // Proactive: a reasoning model (gpt-5 family) never sends a custom
        // temperature — dropped without any 400 round-trip.
        let mut r = ChatRequest::new("gpt-5.6-luna".into(), vec![ChatMessage::new("user", "hi")]);
        r.temperature = Some(0.2);
        assert_eq!(p.build_request_body(&r).temperature, None);
        // Reactive net: a model the heuristic doesn't recognize keeps sending
        // temperature until a 400 teaches us otherwise.
        let mut r2 = ChatRequest::new(
            "some-custom-model".into(),
            vec![ChatMessage::new("user", "hi")],
        );
        r2.temperature = Some(0.2);
        assert_eq!(p.build_request_body(&r2).temperature, Some(0.2));
        DROP_TEMPERATURE.with(|s| s.borrow_mut().insert("some-custom-model".to_string()));
        assert_eq!(p.build_request_body(&r2).temperature, None);
        reset_learned();
    }

    #[test]
    fn forwards_reasoning_effort() {
        reset_learned();
        let p = OpenAiProvider::new("k".into(), None, None).unwrap();
        let mut r = ChatRequest::new("gpt-5.4-mini".into(), vec![ChatMessage::new("user", "hi")]);
        r.reasoning_effort = Some("high".into());
        assert_eq!(
            p.build_request_body(&r).reasoning_effort.as_deref(),
            Some("high")
        );
    }

    #[test]
    fn detects_reasoning_effort_tools_400() {
        assert!(mentions_reasoning_effort_tools_conflict(
            "Function tools with reasoning_effort are not supported for gpt-5.6-luna in \
             /v1/chat/completions. To use function tools, use /v1/responses or set \
             reasoning_effort to 'none'."
        ));
        // Unrelated 400s must not trip it.
        assert!(!mentions_reasoning_effort_tools_conflict(
            "Unsupported parameter: 'temperature' is not supported with this model."
        ));
    }

    #[test]
    fn effort_pinned_to_none_only_when_tools_present() {
        reset_learned();
        let p = OpenAiProvider::new("k".into(), None, None).unwrap();
        FORCE_EFFORT_NONE.with(|s| s.borrow_mut().insert("gpt-5.6-luna".to_string()));
        // With tools: pinned to `none` (the model rejects effort+tools on chat).
        let mut with_tools =
            ChatRequest::new("gpt-5.6-luna".into(), vec![ChatMessage::new("user", "hi")]);
        with_tools.reasoning_effort = Some("high".into());
        with_tools.tools = vec![tool()];
        assert_eq!(
            p.build_request_body(&with_tools)
                .reasoning_effort
                .as_deref(),
            Some("none")
        );
        // WITHOUT tools: the caller's effort is preserved — a tool-free call must
        // not be downgraded just because the model was learned from a tools call.
        let mut no_tools =
            ChatRequest::new("gpt-5.6-luna".into(), vec![ChatMessage::new("user", "hi")]);
        no_tools.reasoning_effort = Some("high".into());
        assert_eq!(
            p.build_request_body(&no_tools).reasoning_effort.as_deref(),
            Some("high")
        );
        reset_learned();
    }

    #[test]
    fn reasoning_model_heuristic() {
        assert!(is_reasoning_model("gpt-5.6-luna"));
        assert!(is_reasoning_model("gpt-5.5"));
        assert!(is_reasoning_model("o3-mini"));
        assert!(is_reasoning_model("o4-mini"));
        assert!(!is_reasoning_model("gpt-5-chat-latest")); // chat alias = non-reasoning
        assert!(!is_reasoning_model("gpt-4o"));
        assert!(!is_reasoning_model("llama-3.3-70b"));
    }

    #[test]
    fn clamp_effort_to_nearest_supported() {
        let sup: Vec<String> = ["none", "low", "medium", "high", "xhigh"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(clamp_effort("minimal", &sup), "low"); // tie none/low → higher tier
        assert_eq!(clamp_effort("max", &sup), "xhigh"); // above the top → clamp down
        assert_eq!(clamp_effort("high", &sup), "high"); // already accepted
        assert_eq!(clamp_effort("high", &[]), "high"); // nothing learned → unchanged
    }

    #[test]
    fn parse_supported_effort_from_400() {
        let msg = "Unsupported value: 'reasoning_effort' does not support 'minimal' with this \
                   model. Supported values are: 'none', 'low', 'medium', 'high', and 'xhigh'.";
        assert!(mentions_unsupported_effort_value(msg));
        // The rejected value ('minimal') is excluded; only the accepted set remains.
        assert_eq!(
            parse_unsupported_effort(msg).unwrap(),
            vec!["none", "low", "medium", "high", "xhigh"]
        );
        // The tools-conflict 400 is a different quirk, not an effort-value error.
        assert!(!mentions_unsupported_effort_value(
            "Function tools with reasoning_effort are not supported for gpt-5.6-luna in \
             /v1/chat/completions. To use function tools, use /v1/responses or set \
             reasoning_effort to 'none'."
        ));
    }

    #[test]
    fn correct_for_400_learns_each_quirk() {
        reset_learned();
        let p = OpenAiProvider::new("k".into(), None, None).unwrap();
        // temperature rejected → dropped
        let mut r = ChatRequest::new("gpt-5.6-luna".into(), vec![ChatMessage::new("user", "hi")]);
        r.temperature = Some(0.2);
        let c = p
            .correct_for_400(
                &r,
                "Unsupported value: 'temperature' does not support 0.2. Only the default (1) \
                 value is supported.",
            )
            .unwrap();
        assert_eq!(c.temperature, None);
        // unsupported effort value → clamped to nearest accepted
        let mut r2 = ChatRequest::new("gpt-5.6-luna".into(), vec![ChatMessage::new("user", "hi")]);
        r2.reasoning_effort = Some("minimal".into());
        let c2 = p
            .correct_for_400(
                &r2,
                "Unsupported value: 'reasoning_effort' does not support 'minimal' with this \
                 model. Supported values are: 'none', 'low', 'medium', 'high', and 'xhigh'.",
            )
            .unwrap();
        assert_eq!(c2.reasoning_effort.as_deref(), Some("low"));
        // effort + tools conflict → pinned to none
        let mut r3 = ChatRequest::new("gpt-5.6-luna".into(), vec![ChatMessage::new("user", "hi")]);
        r3.reasoning_effort = Some("high".into());
        r3.tools = vec![tool()];
        let c3 = p
            .correct_for_400(
                &r3,
                "Function tools with reasoning_effort are not supported ... use /v1/responses \
                 or set reasoning_effort to 'none'.",
            )
            .unwrap();
        assert_eq!(c3.reasoning_effort.as_deref(), Some("none"));
        // compat endpoints don't raise these 400s → no correction
        let compat =
            OpenAiProvider::new("k".into(), Some("http://localhost:1234/v1".into()), None).unwrap();
        assert!(compat
            .correct_for_400(&r, "Unsupported value: 'temperature' does not support 0.2")
            .is_none());
        reset_learned();
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
