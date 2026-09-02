use serde::{Deserialize, Serialize};

use crate::provider::LlmProvider;
use crate::types::{ChatRequest, ChatResponse, LlmError, MessageContent, ToolCall, Usage};

pub struct AnthropicProvider {
    api_key: String,
    default_model: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String, default_model: Option<String>) -> Result<Self, LlmError> {
        Ok(AnthropicProvider {
            api_key,
            default_model: default_model.unwrap_or_else(|| "claude-sonnet-4-6".to_string()),
            client: crate::http::create_client(None)?,
        })
    }

    fn build_request_body(&self, request: &ChatRequest) -> AnthropicRequest {
        let model = crate::provider::resolve_model(&request.model, &self.default_model);
        // NOT a 1:1 map over messages: Anthropic requires every `tool_result`
        // answering one assistant turn to sit in the SINGLE message
        // immediately following it. The agent loop pushes one
        // `ChatMessage::tool_result` per tool call, so emitting one `user`
        // message each made any turn with parallel tool calls 400 with
        // "tool_use ids were found without tool_result blocks immediately
        // after". Claude 4.x emits parallel calls by default, so that was every
        // multi-tool turn. Consecutive tool results are folded into one message.
        // OpenAI and Ollama genuinely do take one message per result, which is
        // why this grouping belongs here and not in the shared request.
        let mut messages: Vec<AnthropicMessage> = Vec::new();
        for m in request.messages.iter().filter(|m| m.role != "system") {
            match m.kind() {
                crate::types::MessageKind::AssistantWithToolCalls(content, tcs) => {
                    // Assistant turn → tool_use content blocks (with optional leading text).
                    let mut blocks: Vec<serde_json::Value> = Vec::new();
                    let text = content.to_text();
                    if !text.is_empty() {
                        blocks.push(serde_json::json!({ "type": "text", "text": text }));
                    }
                    for tc in tcs {
                        blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.arguments,
                        }));
                    }
                    messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: serde_json::Value::Array(blocks),
                    });
                }
                crate::types::MessageKind::ToolResult { id, content, .. } => {
                    // Tool result → a tool_result block keyed by tool_use_id
                    // (Anthropic's correlation mechanism), carried on a user message.
                    let block = serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": id.unwrap_or_default(),
                        "content": content.to_text(),
                    });
                    let appended = messages.last_mut().is_some_and(|last| {
                        if last.role != "user" {
                            return false;
                        }
                        match &mut last.content {
                            // Only extend a message that is ITSELF tool results, so a
                            // genuine user turn is never absorbed.
                            serde_json::Value::Array(blocks)
                                if !blocks.is_empty()
                                    && blocks.iter().all(|b| {
                                        b.get("type").and_then(|t| t.as_str())
                                            == Some("tool_result")
                                    }) =>
                            {
                                blocks.push(block.clone());
                                true
                            }
                            _ => false,
                        }
                    });
                    if !appended {
                        messages.push(AnthropicMessage {
                            role: "user".to_string(),
                            content: serde_json::Value::Array(vec![block]),
                        });
                    }
                }
                crate::types::MessageKind::Other(role, content) => {
                    messages.push(AnthropicMessage {
                        role: role.to_string(),
                        content: serialize_anthropic_content(content),
                    });
                }
            }
        }

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
                retry_after_ms: crate::http::retry_after_ms(resp.headers()),
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
                        thought_signature: None,
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
                retry_after_ms: crate::http::retry_after_ms(resp.headers()),
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
    // index -> a partial tool call (id, name, accumulated partial_json)
    tool_accs: std::collections::BTreeMap<u64, crate::types::PartialToolCall>,
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
                    self.tool_accs.insert(
                        index,
                        crate::types::PartialToolCall {
                            id,
                            name,
                            args: String::new(),
                        },
                    );
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
                            acc.args.push_str(pj);
                        }
                    }
                    None
                }
            }
            Some("content_block_stop") => {
                if let Some(acc) = self.tool_accs.remove(&index) {
                    self.tool_calls.push(acc.finish());
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

    fn into_response(mut self, model: String) -> ChatResponse {
        // Finalize any tool block that never saw a content_block_stop (defensive).
        for acc in std::mem::take(&mut self.tool_accs).into_values() {
            self.tool_calls.push(acc.finish());
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

    /// Anthropic rejects an assistant turn whose `tool_use` blocks are not all
    /// answered in the single message immediately after it. The agent loop
    /// emits one `tool_result` message per call, so a parallel-tool turn used
    /// to serialize as two separate user messages and 400.
    #[test]
    fn parallel_tool_results_are_grouped_into_one_user_message() {
        let p = AnthropicProvider::new("k".into(), Some("claude-x".into())).unwrap();
        let call = |id: &str, name: &str| ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        };
        let mut assistant = ChatMessage::new("assistant", "");
        assistant.tool_calls = vec![call("a", "one"), call("b", "two")];

        let r = ChatRequest::new(
            "claude-x".into(),
            vec![
                ChatMessage::new("user", "go"),
                assistant,
                ChatMessage::tool_result("a", "one", "ra"),
                ChatMessage::tool_result("b", "two", "rb"),
            ],
        );
        let body = p.build_request_body(&r);

        // user, assistant(tool_use x2), user(tool_result x2)
        assert_eq!(
            body.messages.len(),
            3,
            "results must not become two messages"
        );
        let last = &body.messages[2];
        assert_eq!(last.role, "user");
        let blocks = last.content.as_array().expect("tool results are blocks");
        assert_eq!(blocks.len(), 2, "both results belong in the same message");
        let ids: Vec<&str> = blocks
            .iter()
            .map(|b| b["tool_use_id"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(ids, vec!["a", "b"], "each result keeps its own tool_use_id");
        assert!(blocks.iter().all(|b| b["type"] == "tool_result"));
    }

    /// The grouping must not swallow a real user turn that happens to follow
    /// tool results.
    #[test]
    fn a_following_user_turn_stays_its_own_message() {
        let p = AnthropicProvider::new("k".into(), Some("claude-x".into())).unwrap();
        let r = ChatRequest::new(
            "claude-x".into(),
            vec![
                ChatMessage::tool_result("a", "one", "ra"),
                ChatMessage::new("user", "and now this"),
                ChatMessage::tool_result("b", "two", "rb"),
            ],
        );
        let body = p.build_request_body(&r);
        assert_eq!(body.messages.len(), 3);
        assert_eq!(body.messages[1].content.as_str(), Some("and now this"));
    }
}
