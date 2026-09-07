use serde::{Deserialize, Serialize};

/// A content block in a chat message — either text or an image.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        /// Base64-encoded image data
        data: String,
    },
}

/// Message content: either a simple string or multi-modal content blocks.
#[derive(Debug, Clone)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    pub fn text(s: impl Into<String>) -> Self {
        MessageContent::Text(s.into())
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text(s) => Some(s),
            MessageContent::Blocks(blocks) => {
                if blocks.len() == 1 {
                    if let ContentBlock::Text { text } = &blocks[0] {
                        return Some(text);
                    }
                }
                None
            }
        }
    }

    /// Get the text content, concatenating if needed.
    pub fn to_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    pub fn has_images(&self) -> bool {
        match self {
            MessageContent::Text(_) => false,
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: MessageContent,
    /// Tool calls emitted by an assistant turn. When non-empty, this message must
    /// be echoed back to the provider so it can correlate the following tool
    /// results (OpenAI requires the assistant `tool_calls`; Anthropic the
    /// `tool_use` blocks). Empty for ordinary messages.
    pub tool_calls: Vec<ToolCall>,
    /// For a tool-result message (`role == "tool"`): the id of the tool call this
    /// result answers. `None` for ordinary messages. Providers use this to match
    /// the result to the call (`tool_call_id` / `tool_use_id` / `functionResponse`).
    pub tool_call_id: Option<String>,
    /// For a tool-result message: the name of the tool that produced it (Mistral
    /// requires it on tool messages; Gemini keys `functionResponse` by name).
    pub tool_name: Option<String>,
}

impl ChatMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        ChatMessage {
            role: role.into(),
            content: MessageContent::Text(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn with_blocks(role: impl Into<String>, blocks: Vec<ContentBlock>) -> Self {
        ChatMessage {
            role: role.into(),
            content: MessageContent::Blocks(blocks),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// An assistant turn that invoked one or more tools. `content` may be empty
    /// (most providers return empty text alongside tool calls).
    pub fn assistant_with_tool_calls(
        content: impl Into<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        ChatMessage {
            role: "assistant".to_string(),
            content: MessageContent::Text(content.into()),
            tool_calls,
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// A tool-result message correlated to the call it answers (`role == "tool"`).
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        ChatMessage {
            role: "tool".to_string(),
            content: MessageContent::Text(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            tool_name: Some(name.into()),
        }
    }

    /// Classify this message for wire serialization. Computes the triage every
    /// provider used to re-run by hand, so the precedence — a non-empty
    /// `tool_calls` makes an assistant turn regardless of `role`; otherwise a
    /// `role == "tool"` message is a correlated tool result — lives here once.
    pub fn kind(&self) -> MessageKind<'_> {
        if !self.tool_calls.is_empty() {
            MessageKind::AssistantWithToolCalls(&self.content, &self.tool_calls)
        } else if self.role == "tool" {
            // `tool_call_id == None` here still classifies as ToolResult (not
            // Other) — the serializers then emit an empty correlation id. This
            // is the single place to harden if a missing id ever needs to be
            // rejected instead.
            MessageKind::ToolResult {
                id: self.tool_call_id.as_deref(),
                name: self.tool_name.as_deref(),
                content: &self.content,
            }
        } else {
            MessageKind::Other(&self.role, &self.content)
        }
    }
}

/// How a [`ChatMessage`] is interpreted on the wire, per [`ChatMessage::kind`].
/// Carries everything each provider needs for its own serialization, so a
/// provider matches on this instead of re-deriving the triage.
#[derive(Debug, Clone, Copy)]
pub enum MessageKind<'a> {
    /// Assistant turn that invoked tools. `content` may be empty text; the
    /// tool calls carry the id/name/arguments (and Gemini's `thought_signature`).
    AssistantWithToolCalls(&'a MessageContent, &'a [ToolCall]),
    /// A correlated tool result for the named call. `id` is `tool_call_id`
    /// (`None` when absent); `name` is `tool_name`.
    ToolResult {
        id: Option<&'a str>,
        name: Option<&'a str>,
        content: &'a MessageContent,
    },
    /// Any other message: role + content passed through as text.
    Other(&'a str, &'a MessageContent),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    /// Gemini's opaque per-call `thoughtSignature`: captured from the model's
    /// `functionCall` part and echoed back verbatim when the assistant turn is
    /// re-sent on the next round — Gemini 2.5+ rejects a resent tool-call turn
    /// without it (400 INVALID_ARGUMENT). Opaque to every other provider (and
    /// to Sema code); `None` everywhere but Gemini responses. Serde-defaulted
    /// so cassette tapes recorded before this field replay unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// A tool call whose arguments are still streaming in (OpenAI `tool_calls`
/// deltas, Anthropic `input_json_delta` blocks). `name` is assigned — never
/// appended — the first time a delta provides it; `args` accumulates across
/// chunks. Finished with [`PartialToolCall::finish`].
#[derive(Debug, Clone, Default)]
pub struct PartialToolCall {
    pub id: String,
    pub name: String,
    pub args: String,
}

impl PartialToolCall {
    /// Parse the accumulated arguments into a [`ToolCall`], falling back to
    /// `{}` when the arguments are empty or not valid JSON (a truncated
    /// stream must not fail the whole response).
    pub fn finish(self) -> ToolCall {
        let partial = self;
        let arguments = if partial.args.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&partial.args).unwrap_or_else(|_| serde_json::json!({}))
        };
        ToolCall {
            id: partial.id,
            name: partial.name,
            arguments,
            thought_signature: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub system: Option<String>,
    pub tools: Vec<ToolSchema>,
    pub stop_sequences: Vec<String>,
    /// When true, providers that support it will request JSON output mode.
    pub json_mode: bool,
    /// Canonical reasoning effort: `minimal` | `low` | `medium` | `high` | `none`
    /// | `xhigh` (stored raw for forward-compat). Each provider maps it to its
    /// native control (OpenAI `reasoning_effort`, Anthropic extended thinking,
    /// Gemini `thinkingConfig`); providers/models that don't support it ignore it.
    pub reasoning_effort: Option<String>,
    /// Per-call HTTP timeout (milliseconds). `None` uses the client default. Applied as a
    /// per-request reqwest timeout by each provider; ignored by providers that don't send.
    pub timeout_ms: Option<u64>,
}

impl ChatRequest {
    pub fn new(model: String, messages: Vec<ChatMessage>) -> Self {
        ChatRequest {
            model,
            messages,
            max_tokens: Some(4096),
            temperature: None,
            system: None,
            tools: Vec::new(),
            stop_sequences: Vec::new(),
            json_mode: false,
            reasoning_effort: None,
            timeout_ms: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub role: String,
    pub model: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub model: String,
    /// Prompt tokens served from the provider's prompt cache (read hits).
    /// Double-counting note: for OpenAI/Gemini/most OpenAI-compatible providers this
    /// is a SUBSET of `prompt_tokens`; for Anthropic it is SEPARATE from
    /// `input_tokens` (which already excludes cached). 0 when unsupported/absent.
    pub cache_read_input_tokens: u32,
    /// Tokens written to the prompt cache this request. Only Anthropic reports a
    /// distinct creation counter; 0 for everyone else.
    pub cache_creation_input_tokens: u32,
}

/// JSON pointer paths (relative to a provider's usage object) that carry the
/// token counts in that provider's wire format.
pub(crate) struct UsageFields {
    pub prompt: &'static str,
    pub completion: &'static str,
    /// `None` when the provider reports no such counter.
    pub cache_read: Option<&'static str>,
    pub cache_write: Option<&'static str>,
}

impl Usage {
    /// Overwrite the counts `usage` carries; a count `usage` omits keeps its value,
    /// so a streaming caller can fold every chunk that reports usage into one tally.
    pub(crate) fn merge_json(&mut self, usage: &serde_json::Value, fields: &UsageFields) {
        let read = |path: &str| {
            usage
                .pointer(path)
                .and_then(|v| v.as_u64())
                .map(|v| v as u32)
        };
        if let Some(v) = read(fields.prompt) {
            self.prompt_tokens = v;
        }
        if let Some(v) = read(fields.completion) {
            self.completion_tokens = v;
        }
        if let Some(v) = fields.cache_read.and_then(read) {
            self.cache_read_input_tokens = v;
        }
        if let Some(v) = fields.cache_write.and_then(read) {
            self.cache_creation_input_tokens = v;
        }
    }

    pub fn total_tokens(&self) -> u32 {
        self.prompt_tokens + self.completion_tokens
    }
}

#[derive(Debug, Clone)]
pub struct EmbedRequest {
    pub texts: Vec<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EmbedResponse {
    pub embeddings: Vec<Vec<f64>>,
    pub model: String,
    pub usage: Usage,
}

/// Canonical rerank request (one shape Sema produces; each provider translates it).
#[derive(Debug, Clone)]
pub struct RerankRequest {
    pub query: String,
    pub documents: Vec<String>,
    /// Keep only the top-K most relevant (provider-side); `None` returns all, reordered.
    pub top_k: Option<usize>,
    pub model: Option<String>,
}

/// One reranked result: an index back into the original `documents` plus its relevance score.
#[derive(Debug, Clone)]
pub struct RerankResult {
    pub index: usize,
    pub score: f64,
}

/// Rerank response: `results` are sorted by descending relevance (highest first).
#[derive(Debug, Clone)]
pub struct RerankResponse {
    pub results: Vec<RerankResult>,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("API error: {status} - {message}")]
    Api { status: u16, message: String },
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Config error: {0}")]
    Config(String),
    #[error("Rate limited: retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_constructor() {
        let content = MessageContent::text("hello");
        assert!(matches!(content, MessageContent::Text(s) if s == "hello"));
    }

    #[test]
    fn as_text_for_text_variant() {
        let content = MessageContent::Text("hello".into());
        assert_eq!(content.as_text(), Some("hello"));
    }

    #[test]
    fn as_text_for_single_text_block() {
        let content = MessageContent::Blocks(vec![ContentBlock::Text {
            text: "hello".into(),
        }]);
        assert_eq!(content.as_text(), Some("hello"));
    }

    #[test]
    fn as_text_for_multi_block_returns_none() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "hello".into(),
            },
            ContentBlock::Text {
                text: "world".into(),
            },
        ]);
        assert_eq!(content.as_text(), None);
    }

    #[test]
    fn as_text_for_image_block_returns_none() {
        let content = MessageContent::Blocks(vec![ContentBlock::Image {
            media_type: Some("image/png".into()),
            data: "abc".into(),
        }]);
        assert_eq!(content.as_text(), None);
    }

    #[test]
    fn to_text_concatenates_text_blocks_ignores_images() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "hello ".into(),
            },
            ContentBlock::Image {
                media_type: None,
                data: "img".into(),
            },
            ContentBlock::Text {
                text: "world".into(),
            },
        ]);
        assert_eq!(content.to_text(), "hello world");
    }

    #[test]
    fn to_text_for_text_variant() {
        let content = MessageContent::Text("simple".into());
        assert_eq!(content.to_text(), "simple");
    }

    #[test]
    fn has_images_false_for_text() {
        assert!(!MessageContent::Text("hi".into()).has_images());
    }

    #[test]
    fn has_images_true_for_blocks_with_image() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Text { text: "hi".into() },
            ContentBlock::Image {
                media_type: None,
                data: "data".into(),
            },
        ]);
        assert!(content.has_images());
    }

    #[test]
    fn has_images_false_for_text_only_blocks() {
        let content = MessageContent::Blocks(vec![ContentBlock::Text { text: "hi".into() }]);
        assert!(!content.has_images());
    }

    #[test]
    fn chat_message_new() {
        let msg = ChatMessage::new("user", "hello");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content.as_text(), Some("hello"));
    }

    #[test]
    fn chat_message_with_blocks() {
        let msg =
            ChatMessage::with_blocks("assistant", vec![ContentBlock::Text { text: "hi".into() }]);
        assert_eq!(msg.role, "assistant");
        assert!(matches!(msg.content, MessageContent::Blocks(_)));
    }

    #[test]
    fn chat_request_defaults() {
        let req = ChatRequest::new("model".into(), vec![]);
        assert_eq!(req.model, "model");
        assert_eq!(req.max_tokens, Some(4096));
        assert!(req.temperature.is_none());
        assert!(req.system.is_none());
        assert!(req.tools.is_empty());
        assert!(req.stop_sequences.is_empty());
        assert!(!req.json_mode);
    }

    #[test]
    fn usage_total_tokens() {
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            model: "m".into(),
            ..Default::default()
        };
        assert_eq!(usage.total_tokens(), 150);
    }

    #[test]
    fn llm_error_display() {
        assert_eq!(
            LlmError::Http("timeout".into()).to_string(),
            "HTTP error: timeout"
        );
        assert_eq!(
            LlmError::Api {
                status: 429,
                message: "too many".into()
            }
            .to_string(),
            "API error: 429 - too many"
        );
        assert_eq!(
            LlmError::Parse("bad json".into()).to_string(),
            "Parse error: bad json"
        );
        assert_eq!(
            LlmError::Config("missing key".into()).to_string(),
            "Config error: missing key"
        );
        assert_eq!(
            LlmError::RateLimited {
                retry_after_ms: 5000
            }
            .to_string(),
            "Rate limited: retry after 5000ms"
        );
    }

    #[test]
    fn message_kind_triage_precedence() {
        // A non-empty tool_calls wins over the role tag.
        let assistant = ChatMessage::assistant_with_tool_calls(
            "thinking",
            vec![ToolCall {
                id: "c1".into(),
                name: "get_weather".into(),
                arguments: serde_json::json!({}),
                thought_signature: None,
            }],
        );
        assert!(matches!(
            assistant.kind(),
            MessageKind::AssistantWithToolCalls(c, tcs)
                if c.as_text() == Some("thinking") && tcs.len() == 1
        ));

        // role "tool" (no tool_calls) is a correlated tool result.
        let result = ChatMessage::tool_result("c1", "get_weather", "42");
        assert!(matches!(
            result.kind(),
            MessageKind::ToolResult {
                id: Some("c1"),
                name: Some("get_weather"),
                ..
            }
        ));

        // Anything else is a plain pass-through; a tool role with no id keeps
        // the Option None so providers that distinguish None from "" still can.
        let plain = ChatMessage::new("user", "hi");
        assert!(matches!(plain.kind(), MessageKind::Other("user", c) if c.as_text() == Some("hi")));
    }

    #[test]
    fn message_kind_tool_role_without_id_still_classifies_as_tool_result() {
        // role == "tool" with no tool_call_id still classifies as ToolResult
        // (not Other) — id is None, and serializers emit an empty correlation
        // id downstream. Pinning this pre-existing behavior.
        let orphan = ChatMessage::new("tool", "42");
        assert!(matches!(
            orphan.kind(),
            MessageKind::ToolResult {
                id: None,
                name: None,
                ..
            }
        ));
    }

    #[test]
    fn partial_tool_call_finish_parses_or_falls_back() {
        // Valid JSON argument fragments are parsed.
        let done = PartialToolCall {
            id: "c1".into(),
            name: "f".into(),
            args: r#"{"a":1}"#.into(),
        }
        .finish();
        assert_eq!(done.id, "c1");
        assert_eq!(done.name, "f");
        assert_eq!(done.arguments, serde_json::json!({"a":1}));

        // Empty / whitespace-only arguments fall back to {}.
        let empty = PartialToolCall {
            id: "c2".into(),
            name: "g".into(),
            args: "   ".into(),
        }
        .finish();
        assert_eq!(empty.arguments, serde_json::json!({}));

        // Truncated (invalid) JSON also falls back to {}, not an error.
        let trunc = PartialToolCall {
            id: "c3".into(),
            name: "h".into(),
            args: r#"{"a":"#.into(),
        }
        .finish();
        assert_eq!(trunc.arguments, serde_json::json!({}));
    }
}
