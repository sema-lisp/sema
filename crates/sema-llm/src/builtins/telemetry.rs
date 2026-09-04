use super::*;

/// Send a ChatRequest via the default provider with caching, fallback, and rate-limit retry.
/// Build the OTel `ResponseFacts` snapshot from a served response. Cost is priced as
/// served by `provider` (matches `track_usage`).
pub(super) fn response_facts(provider: &str, resp: &ChatResponse) -> sema_otel::ResponseFacts {
    let split = pricing::calculate_cost_split_for(provider, &resp.usage);
    sema_otel::ResponseFacts {
        input_tokens: resp.usage.prompt_tokens,
        output_tokens: resp.usage.completion_tokens,
        cache_read_input_tokens: resp.usage.cache_read_input_tokens,
        cache_creation_input_tokens: resp.usage.cache_creation_input_tokens,
        response_model: resp.model.clone(),
        finish_reason: resp.stop_reason.clone(),
        cost_usd: pricing::calculate_cost_for(provider, &resp.usage),
        cost_prompt_usd: split.map(|(p, _)| p),
        cost_completion_usd: split.map(|(_, c)| c),
        cache_hit: resp.stop_reason.as_deref() == Some("cache_hit"),
    }
}

/// Per-message content cap (chars) for opt-in content capture, applied BEFORE JSON
/// encoding so truncation never splits the JSON.
pub(super) const CONTENT_FIELD_MAX: usize = 8192;

pub(super) fn truncate_content(s: &str) -> String {
    if s.len() <= CONTENT_FIELD_MAX {
        return s.to_string();
    }
    // Guard and truncate both in BYTES (the stated intent is bounding attribute size);
    // back off to the nearest char boundary so the slice is valid UTF-8.
    let mut end = CONTENT_FIELD_MAX;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated]", &s[..end])
}

/// Encode chat messages as the GenAI structured-message JSON array
/// `[{"role":..,"parts":[{"type":"text","content":..}]}]` for opt-in content capture.
pub(super) fn messages_json(messages: &[ChatMessage]) -> String {
    let arr: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "role": m.role,
                "parts": [{"type": "text", "content": truncate_content(&m.content.to_text())}],
            })
        })
        .collect();
    serde_json::Value::Array(arr).to_string()
}

/// Encode a single role/content turn as the structured-message JSON array.
pub(super) fn content_json(role: &str, content: &str) -> String {
    serde_json::json!([{
        "role": role,
        "parts": [{"type": "text", "content": truncate_content(content)}],
    }])
    .to_string()
}

/// Conversation / session / user identity threaded into the agent + completion spans.
#[derive(Default, Clone)]
pub(super) struct ConvScope {
    pub(super) conversation: Option<String>,
    pub(super) session: Option<String>,
    pub(super) user: Option<String>,
}

impl ConvScope {
    /// Read `:conversation-id` / `:session-id` / `:user-id` from an options map.
    pub(super) fn from_opts(opts: Option<&Rc<BTreeMap<Value, Value>>>) -> Self {
        let get = |k: &str| {
            opts.and_then(|o| o.get(&Value::keyword(k)))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        };
        ConvScope {
            conversation: get("conversation-id"),
            session: get("session-id"),
            user: get("user-id"),
        }
    }

    /// Open a telemetry scope when ANY id was supplied (a missing conversation id is
    /// generated, so `:session-id`/`:user-id` alone still take effect). Returns `None`
    /// when nothing was supplied (the callee will generate a fresh conversation id).
    pub(super) fn open(&self) -> Option<sema_otel::ConversationGuard> {
        if self.conversation.is_none() && self.session.is_none() && self.user.is_none() {
            return None;
        }
        let cid = self
            .conversation
            .clone()
            .unwrap_or_else(sema_otel::new_conversation_id);
        Some(sema_otel::set_conversation_scope(
            &cid,
            self.session.as_deref(),
            self.user.as_deref(),
        ))
    }
}

/// Classify an `LlmError` for the `error.type` span attribute.
pub(super) fn llm_error_kind(e: &crate::types::LlmError) -> &'static str {
    use crate::types::LlmError::*;
    match e {
        RateLimited { .. } => "rate_limited",
        Api { status, .. } if *status >= 500 => "server_error",
        Api { .. } => "api_error",
        Http(_) => "network_error",
        Parse(_) => "parse_error",
        Config(_) => "config_error",
    }
}

thread_local! {
    /// Per-call user observability tags, set by an LLM builtin from its options map and
    /// read where the span is constructed (deeper in `do_complete` / `run_tool_loop`).
    pub(super) static CALL_TAGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Per-call user observability metadata (string -> string), same lifecycle as tags.
    pub(super) static CALL_META: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// RAII install of per-call user tags/metadata. Saves and restores the previous values
/// on drop so a nested LLM call (e.g. `llm/complete` inside an agent tool) can't wipe an
/// outer call's telemetry.
pub(super) struct CallTelemetry {
    prev_tags: Vec<String>,
    prev_meta: Vec<(String, String)>,
}

impl Drop for CallTelemetry {
    fn drop(&mut self) {
        CALL_TAGS.with(|t| *t.borrow_mut() = std::mem::take(&mut self.prev_tags));
        CALL_META.with(|m| *m.borrow_mut() = std::mem::take(&mut self.prev_meta));
    }
}

/// Install per-call tags/metadata parsed from a call's options map. Returns `None` (no
/// guard, parent telemetry inherited) when neither `:tags` nor `:metadata` is present.
pub(super) fn install_call_telemetry(
    opts: Option<&Rc<BTreeMap<Value, Value>>>,
) -> Option<CallTelemetry> {
    let opts = opts?;
    let tags = get_opt_string_list(opts, "tags");
    let meta = get_opt_str_map(opts, "metadata");
    if tags.is_empty() && meta.is_empty() {
        return None;
    }
    let prev_tags = CALL_TAGS.with(|t| std::mem::replace(&mut *t.borrow_mut(), tags));
    let prev_meta = CALL_META.with(|m| std::mem::replace(&mut *m.borrow_mut(), meta));
    Some(CallTelemetry {
        prev_tags,
        prev_meta,
    })
}

/// Attach the active per-call tags/metadata to an LLM span.
pub(super) fn apply_call_telemetry_llm(span: &sema_otel::LlmSpan) {
    CALL_TAGS.with(|t| {
        let t = t.borrow();
        if !t.is_empty() {
            span.set_tags(&t);
        }
    });
    CALL_META.with(|m| {
        let m = m.borrow();
        if !m.is_empty() {
            span.set_metadata(&m);
        }
    });
}

/// Attach the active per-call tags/metadata to an agent span.
pub(super) fn apply_call_telemetry_agent(span: &sema_otel::AgentSpan) {
    CALL_TAGS.with(|t| {
        let t = t.borrow();
        if !t.is_empty() {
            span.set_tags(&t);
        }
    });
    CALL_META.with(|m| {
        let m = m.borrow();
        if !m.is_empty() {
            span.set_metadata(&m);
        }
    });
}
