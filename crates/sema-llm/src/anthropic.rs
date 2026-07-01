use serde::{Deserialize, Serialize};

use crate::provider::LlmProvider;
use crate::types::{ChatRequest, ChatResponse, LlmError, MessageContent, ToolCall, Usage};

pub struct AnthropicProvider {
    api_key: String,
    default_model: String,
    client: reqwest::Client,
    runtime: crate::http::BlockingRuntime,
}

impl AnthropicProvider {
    pub fn new(api_key: String, default_model: Option<String>) -> Result<Self, LlmError> {
        let runtime = crate::http::create_runtime()?;
        Ok(AnthropicProvider {
            api_key,
            default_model: default_model.unwrap_or_else(|| "claude-sonnet-4-6".to_string()),
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

    fn build_request_body(&self, request: &ChatRequest) -> AnthropicRequest {
        let model = self.resolve_model(&request.model);
        let messages: Vec<AnthropicMessage> = request
            .messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| {
                if !m.tool_calls.is_empty() {
                    // Assistant turn → tool_use content blocks (with optional leading text).
                    let mut blocks: Vec<serde_json::Value> = Vec::new();
                    let text = m.content.to_text();
                    if !text.is_empty() {
                        blocks.push(serde_json::json!({ "type": "text", "text": text }));
                    }
                    for tc in &m.tool_calls {
                        blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.arguments,
                        }));
                    }
                    AnthropicMessage {
                        role: "assistant".to_string(),
                        content: serde_json::Value::Array(blocks),
                    }
                } else if m.role == "tool" {
                    // Tool result → a USER message carrying a tool_result block keyed
                    // by tool_use_id (Anthropic's correlation mechanism).
                    AnthropicMessage {
                        role: "user".to_string(),
                        content: serde_json::json!([{
                            "type": "tool_result",
                            "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                            "content": m.content.to_text(),
                        }]),
                    }
                } else {
                    AnthropicMessage {
                        role: m.role.clone(),
                        content: serialize_anthropic_content(&m.content),
                    }
                }
            })
            .collect();

        let system = request.system.clone().or_else(|| {
            request
                .messages
                .iter()
                .find(|m| m.role == "system")
                .map(|m| m.content.to_text())
        });

        let tools: Vec<AnthropicTool> = request
            .tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
            })
            .collect();

        // Canonical reasoning_effort → Anthropic extended thinking. When enabled,
        // Anthropic requires max_tokens > budget_tokens and temperature unset
        // (defaults to 1), so we keep the caller's max_tokens as output room on top
        // of the thinking budget and drop temperature.
        let output_room = request.max_tokens.unwrap_or(4096);
        let mut max_tokens = output_room;
        let mut temperature = request.temperature;
        let thinking = request
            .reasoning_effort
            .as_deref()
            .and_then(anthropic_thinking_budget)
            .map(|budget| {
                max_tokens = budget + output_room;
                temperature = None;
                ThinkingConfig {
                    kind: "enabled",
                    budget_tokens: budget,
                }
            });

        AnthropicRequest {
            model,
            messages,
            max_tokens,
            temperature,
            system,
            tools,
            stop_sequences: request.stop_sequences.clone(),
            stream: false,
            thinking,
        }
    }

    async fn complete_async(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let body = self.build_request_body(&request);

        let resp = crate::http::with_timeout(
            self.client.post("https://api.anthropic.com/v1/messages"),
            request.timeout_ms,
        )
        .header("x-api-key", &self.api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
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
            if let Ok(err) = serde_json::from_str::<AnthropicError>(&text) {
                return Err(LlmError::Api {
                    status,
                    message: err.error.message,
                });
            }
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }

        let api_resp: AnthropicResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        for block in &api_resp.content {
            match block {
                ContentBlock::Text { text } => {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(text);
                }
                ContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: input.clone(),
                    });
                }
                // Thinking / redacted_thinking / unknown blocks: ignore.
                ContentBlock::Other => {}
            }
        }

        Ok(ChatResponse {
            content,
            role: api_resp.role,
            model: api_resp.model.clone(),
            tool_calls,
            usage: Usage {
                prompt_tokens: api_resp.usage.input_tokens,
                completion_tokens: api_resp.usage.output_tokens,
                model: api_resp.model,
                cache_read_input_tokens: api_resp.usage.cache_read_input_tokens,
                cache_creation_input_tokens: api_resp.usage.cache_creation_input_tokens,
            },
            stop_reason: api_resp.stop_reason,
        })
    }

    async fn stream_complete_async(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>,
    ) -> Result<ChatResponse, LlmError> {
        let mut body = self.build_request_body(&request);
        body.stream = true;
        let model_name = body.model.clone();

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
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
            if let Ok(err) = serde_json::from_str::<AnthropicError>(&text) {
                return Err(LlmError::Api {
                    status,
                    message: err.error.message,
                });
            }
            return Err(LlmError::Api {
                status,
                message: text,
            });
        }

        let mut acc = AnthropicStreamAccum::default();
        crate::sse::parse_sse_stream(resp, |data| {
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                if let Some(text) = acc.on_event(&event) {
                    on_chunk(&text)?;
                }
            }
            Ok(())
        })
        .await?;

        Ok(acc.into_response(model_name))
    }
}

/// Incremental accumulator for an Anthropic streaming (`stream=true`) response.
///
/// Kept as a plain struct rather than inline in the async loop so the tool_use
/// assembly — the exact path that once dropped streamed tool calls, producing an
/// empty turn — is unit-testable without a socket. Tool calls stream as
/// `content_block_start {type:tool_use,id,name}` → repeated
/// `content_block_delta {input_json_delta, partial_json}` (the arguments JSON
/// arrives in fragments) → `content_block_stop`; blocks are keyed by `index` so
/// several parallel tool calls in one turn don't interleave.
#[derive(Default)]
struct AnthropicStreamAccum {
    content: String,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_input_tokens: u32,
    cache_creation_input_tokens: u32,
    stop_reason: Option<String>,
    // index -> (id, name, partial-json)
    tool_accs: std::collections::BTreeMap<u64, (String, String, String)>,
    tool_calls: Vec<ToolCall>,
}

impl AnthropicStreamAccum {
    /// Process one decoded SSE event; return any text delta to forward to the
    /// caller's on_chunk callback (`None` for non-text events).
    fn on_event(&mut self, event: &serde_json::Value) -> Option<String> {
        let index = event.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
        match event.get("type").and_then(|t| t.as_str()) {
            Some("message_start") => {
                if let Some(usage) = event.pointer("/message/usage") {
                    self.input_tokens = usage
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    // Anthropic reports cache tokens SEPARATELY from input_tokens.
                    self.cache_read_input_tokens = usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    self.cache_creation_input_tokens = usage
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                }
                None
            }
            Some("content_block_start") => {
                if event
                    .pointer("/content_block/type")
                    .and_then(|t| t.as_str())
                    == Some("tool_use")
                {
                    let id = event
                        .pointer("/content_block/id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = event
                        .pointer("/content_block/name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.tool_accs.insert(index, (id, name, String::new()));
                }
                None
            }
            Some("content_block_delta") => {
                if let Some(s) = event.pointer("/delta/text").and_then(|t| t.as_str()) {
                    self.content.push_str(s);
                    Some(s.to_string())
                } else {
                    if let Some(pj) = event
                        .pointer("/delta/partial_json")
                        .and_then(|t| t.as_str())
                    {
                        if let Some(acc) = self.tool_accs.get_mut(&index) {
                            acc.2.push_str(pj);
                        }
                    }
                    None
                }
            }
            Some("content_block_stop") => {
                if let Some(acc) = self.tool_accs.remove(&index) {
                    self.tool_calls.push(Self::finalize(acc));
                }
                None
            }
            Some("message_delta") => {
                if let Some(usage) = event.get("usage") {
                    self.output_tokens = usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                }
                if let Some(sr) = event.pointer("/delta/stop_reason") {
                    self.stop_reason = sr.as_str().map(|s| s.to_string());
                }
                None
            }
            _ => None,
        }
    }

    fn finalize(acc: (String, String, String)) -> ToolCall {
        let (id, name, json) = acc;
        let arguments = if json.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&json).unwrap_or_else(|_| serde_json::json!({}))
        };
        ToolCall {
            id,
            name,
            arguments,
        }
    }

    fn into_response(mut self, model: String) -> ChatResponse {
        // Finalize any tool block that never saw a content_block_stop (defensive).
        for acc in std::mem::take(&mut self.tool_accs).into_values() {
            self.tool_calls.push(Self::finalize(acc));
        }
        ChatResponse {
            content: self.content,
            role: "assistant".to_string(),
            model: model.clone(),
            tool_calls: self.tool_calls,
            usage: Usage {
                prompt_tokens: self.input_tokens,
                completion_tokens: self.output_tokens,
                model,
                cache_read_input_tokens: self.cache_read_input_tokens,
                cache_creation_input_tokens: self.cache_creation_input_tokens,
            },
            stop_reason: self.stop_reason,
        }
    }
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop_sequences: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
}

#[derive(Serialize)]
struct ThinkingConfig {
    #[serde(rename = "type")]
    kind: &'static str,
    budget_tokens: u32,
}

/// Map canonical reasoning effort → Anthropic extended-thinking budget (tokens).
/// `minimal`/`none`/unrecognized disable thinking (None).
fn anthropic_thinking_budget(effort: &str) -> Option<u32> {
    match effort.to_lowercase().as_str() {
        "low" => Some(1024),
        "medium" => Some(4096),
        "high" => Some(12000),
        "xhigh" | "max" => Some(24000),
        _ => None,
    }
}

#[derive(Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
    model: String,
    role: String,
    usage: AnthropicUsage,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// `thinking` / `redacted_thinking` (extended thinking) and any future block
    /// types — tolerated and ignored so the response still decodes.
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    // Prompt-cache accounting (Anthropic reports these SEPARATELY from input_tokens).
    #[serde(default)]
    cache_creation_input_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

#[derive(Deserialize)]
struct AnthropicError {
    error: AnthropicErrorDetail,
}

#[derive(Deserialize)]
struct AnthropicErrorDetail {
    message: String,
    #[serde(rename = "type")]
    _error_type: Option<String>,
}

impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
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

fn serialize_anthropic_content(content: &MessageContent) -> serde_json::Value {
    match content {
        MessageContent::Text(s) => serde_json::Value::String(s.clone()),
        MessageContent::Blocks(blocks) => {
            let arr: Vec<serde_json::Value> = blocks
                .iter()
                .map(|b| match b {
                    crate::types::ContentBlock::Text { text } => serde_json::json!({
                        "type": "text",
                        "text": text
                    }),
                    crate::types::ContentBlock::Image { media_type, data } => serde_json::json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type.as_deref().unwrap_or("application/octet-stream"),
                            "data": data
                        }
                    }),
                })
                .collect();
            serde_json::Value::Array(arr)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, ChatRequest};

    /// Regression: a streamed tool call must be assembled from its SSE fragments
    /// (start → input_json_delta × N → stop), not dropped. Dropping it made the
    /// agent loop see an empty turn and skip the tool entirely.
    #[test]
    fn streaming_assembles_tool_use_and_text() {
        let events = [
            serde_json::json!({"type":"message_start","message":{"usage":{"input_tokens":10}}}),
            serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"text"}}),
            serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me look. "}}),
            serde_json::json!({"type":"content_block_stop","index":0}),
            serde_json::json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01xyz","name":"list-dir"}}),
            serde_json::json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
            serde_json::json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\".\"}"}}),
            serde_json::json!({"type":"content_block_stop","index":1}),
            serde_json::json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}),
        ];
        let mut acc = AnthropicStreamAccum::default();
        let mut streamed = String::new();
        for e in &events {
            if let Some(t) = acc.on_event(e) {
                streamed.push_str(&t);
            }
        }
        let resp = acc.into_response("claude-x".into());

        assert_eq!(streamed, "Let me look. ", "text deltas still stream");
        assert_eq!(resp.content, "Let me look. ");
        assert_eq!(resp.tool_calls.len(), 1, "the tool call must survive");
        let tc = &resp.tool_calls[0];
        assert_eq!(tc.id, "toolu_01xyz");
        assert_eq!(tc.name, "list-dir");
        assert_eq!(tc.arguments, serde_json::json!({"path": "."}));
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(resp.usage.prompt_tokens, 10);
        assert_eq!(resp.usage.completion_tokens, 7);
    }

    #[test]
    fn budget_mapping() {
        assert_eq!(anthropic_thinking_budget("low"), Some(1024));
        assert_eq!(anthropic_thinking_budget("medium"), Some(4096));
        assert_eq!(anthropic_thinking_budget("high"), Some(12000));
        assert_eq!(anthropic_thinking_budget("none"), None);
        assert_eq!(anthropic_thinking_budget("minimal"), None);
    }

    #[test]
    fn high_effort_enables_thinking_and_relaxes_constraints() {
        let p = AnthropicProvider::new("k".into(), Some("claude-x".into())).unwrap();
        let mut r = ChatRequest::new("claude-x".into(), vec![ChatMessage::new("user", "hi")]);
        r.max_tokens = Some(1000);
        r.temperature = Some(0.5);
        r.reasoning_effort = Some("high".into());
        let body = p.build_request_body(&r);
        let t = body.thinking.expect("thinking enabled for high");
        assert_eq!(t.budget_tokens, 12000);
        assert!(
            body.max_tokens > t.budget_tokens,
            "max_tokens ({}) must exceed thinking budget ({})",
            body.max_tokens,
            t.budget_tokens
        );
        assert_eq!(body.temperature, None, "temperature dropped with thinking");
    }

    #[test]
    fn none_effort_disables_thinking() {
        let p = AnthropicProvider::new("k".into(), Some("claude-x".into())).unwrap();
        let mut r = ChatRequest::new("claude-x".into(), vec![ChatMessage::new("user", "hi")]);
        r.reasoning_effort = Some("none".into());
        assert!(p.build_request_body(&r).thinking.is_none());
    }
}
