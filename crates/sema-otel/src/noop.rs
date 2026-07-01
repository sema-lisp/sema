//! WASM (browser) build: pure compile-out no-op (Decision #11).
//!
//! There is no span *source* in wasm — `sema-llm` and the LLM/GenAI builtins are all
//! `cfg(not(wasm32))`-excluded — and the export machinery (threads, tokio, reqwest,
//! `std::fs`, `Instant`) is structurally unavailable. So the facade mirrors the
//! native public API exactly but every operation is inert. None of
//! `opentelemetry-otlp` / `tokio` / `std::fs` is linked.

use crate::ResponseFacts;

/// No-op guard. Present so call sites are identical across targets.
pub struct OtelGuard;

/// Always `None` on wasm — there is nothing to install.
pub fn init_from_env() -> Option<OtelGuard> {
    None
}

/// No-op on wasm.
pub fn use_host_global() {}

/// Programmatic OTel config (`otel/configure`). Mirrors the native struct so the stdlib
/// builtin compiles for wasm; every field is inert here.
#[derive(Debug, Clone, Default)]
pub struct OtelConfig {
    pub endpoint: Option<String>,
    pub file: Option<String>,
    pub protocol: Option<String>,
    pub headers: Option<String>,
    pub service_name: Option<String>,
    pub environment: Option<String>,
    pub release: Option<String>,
    pub capture_content: Option<bool>,
}

/// Always `false` on wasm — there is nothing to install.
pub fn configure(_cfg: &OtelConfig) -> bool {
    false
}

/// Backend compat is native-only; always inactive on wasm.
pub fn compat_active() -> bool {
    false
}

/// Content capture is native-only; always off on wasm.
pub fn content_capture_enabled() -> bool {
    false
}

/// Embedded telemetry mode. `OwnProvider` is omitted on wasm (no `SdkTracerProvider`
/// exists there); the whole LLM/otel surface is native-only anyway.
pub enum TelemetryMode {
    Off,
    UseHostGlobal,
    FromEnv,
}

pub fn activate(_mode: TelemetryMode) -> Option<OtelGuard> {
    None
}

pub struct ConversationGuard;

pub fn set_conversation_scope(
    _conversation_id: &str,
    _session_id: Option<&str>,
    _user_id: Option<&str>,
) -> ConversationGuard {
    ConversationGuard
}

pub fn current_conversation_id() -> Option<String> {
    None
}

pub fn new_conversation_id() -> String {
    String::new()
}

/// wasm no-op: there is no real otel context to swap, so this is an empty marker.
#[derive(Default)]
pub struct OtelTaskCtx;

pub fn take_task_otel() -> OtelTaskCtx {
    OtelTaskCtx
}

pub fn install_task_otel(_ctx: OtelTaskCtx) -> OtelTaskCtx {
    OtelTaskCtx
}

pub fn current_conversation_scope() -> OtelTaskCtx {
    OtelTaskCtx
}

/// wasm no-op: the scheduler never offloads on wasm, so no callbacks are wired.
pub fn register_task_callbacks() {}

pub struct LlmSpan;

pub fn llm_span_detached(_op: &'static str) -> LlmSpan {
    LlmSpan
}

pub fn llm_span(_op: &'static str) -> LlmSpan {
    LlmSpan
}

impl LlmSpan {
    pub fn set_request(
        &self,
        _temperature: Option<f64>,
        _max_tokens: Option<u32>,
        _stop_sequences: &[String],
        _reasoning_effort: Option<&str>,
    ) {
    }
    pub fn set_output_type(&self, _json: bool) {}
    pub fn set_tools(&self, _tools: &[crate::ToolView]) {}
    pub fn set_embedding_input(&self, _texts: &[String]) {}
    pub fn set_trace_io(&self, _input: &str, _output: &str) {}
    pub fn set_tags(&self, _tags: &[String]) {}
    pub fn set_metadata(&self, _meta: &[(String, String)]) {}
    pub fn mark_first_token(&self) {}
    pub fn set_dispatch(&self, _sema_provider: &str, _request_model: &str) {}
    pub fn set_response(&self, _facts: &ResponseFacts) {}
    pub fn set_conversation_id(&self, _id: &str) {}
    pub fn set_messages(&self, _input: &str, _output: &str, _system: Option<&str>) {}
    pub fn record_error(&self, _kind: &str, _msg: &str) {}
    pub fn entered<R>(&self, f: impl FnOnce() -> R) -> R {
        f()
    }
}

pub struct ToolSpan;

pub fn tool_span(_name: &str, _call_id: &str, _description: Option<&str>) -> ToolSpan {
    ToolSpan
}

impl ToolSpan {
    pub fn set_conversation_id(&self, _id: &str) {}
    pub fn set_tool_io(&self, _args_json: &str, _result: &str) {}
    pub fn record_error(&self, _kind: &str, _msg: &str) {}
}

pub struct AgentSpan;

pub fn agent_span(_name: Option<&str>) -> AgentSpan {
    AgentSpan
}

impl AgentSpan {
    pub fn set_conversation_id(&self, _id: &str) {}
    pub fn set_trace_io(&self, _input: &str, _output: &str) {}
    pub fn set_tags(&self, _tags: &[String]) {}
    pub fn set_metadata(&self, _meta: &[(String, String)]) {}
    pub fn record_error(&self, _kind: &str, _msg: &str) {}
}

pub struct RetrieverSpan;

pub fn retriever_span(_query_dims: usize, _k: usize) -> RetrieverSpan {
    RetrieverSpan
}

impl RetrieverSpan {
    pub fn set_documents(&self, _docs: &[(String, String, f64)]) {}
    pub fn record_error(&self, _kind: &str, _msg: &str) {}
}

pub struct RerankerSpan;

pub fn reranker_span(_query: &str, _model: &str, _top_k: Option<usize>) -> RerankerSpan {
    RerankerSpan
}

impl RerankerSpan {
    pub fn set_input(&self, _docs: &[String]) {}
    pub fn set_output(&self, _docs: &[(String, f64)]) {}
    pub fn record_error(&self, _kind: &str, _msg: &str) {}
}

pub struct RetrySpan;

pub fn retry_span(_attempt: u32) -> RetrySpan {
    RetrySpan
}

impl RetrySpan {
    pub fn record_error(&self, _kind: &str, _msg: &str) {}
    pub fn set_wait_ms(&self, _ms: u64) {}
}

pub struct VmSpan;

pub fn vm_span(_name: &str) -> VmSpan {
    VmSpan
}

impl VmSpan {
    pub fn set_str(&self, _key: &'static str, _val: &str) {}
}

pub fn add_event(_name: &str, _attrs: Vec<(String, String)>) {}

// Sema-native tracing surface — inert on wasm.
pub fn set_current_attr(_key: &str, _val: crate::AttrValue) {}
pub fn set_current_attrs(_attrs: Vec<(String, crate::AttrValue)>) {}
pub fn set_current_status(_error: Option<&str>) {}
pub fn set_current_llm_usage(_input: u32, _output: u32, _cost: Option<f64>) {}

pub fn user_span(
    _name: &str,
    _kind: crate::SemaSpanKind,
    _attrs: Vec<(String, crate::AttrValue)>,
) -> VmSpan {
    VmSpan
}

pub fn user_llm_span(
    _model: &str,
    _provider: &str,
    _operation: &str,
    _attrs: Vec<(String, crate::AttrValue)>,
) -> VmSpan {
    VmSpan
}
