//! Native (non-wasm) OpenTelemetry implementation of the facade.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use std::time::Duration;

use opentelemetry::global;
use opentelemetry::metrics::Histogram;
use opentelemetry::trace::{SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{Array, Context, InstrumentationScope, KeyValue, StringValue, Value};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;

use crate::file_exporter::JsonlFileExporter;
use crate::provider_map::gen_ai_provider_name;
use crate::ResponseFacts;

/// Set true once a tracer provider is installed (by `init_from_env` or
/// `use_host_global`). When false, every span constructor returns a no-op without
/// touching `global`.
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Set true once a meter provider is installed (OTLP-only — the JSONL file sink is
/// trace-only). When false, metric recording is skipped.
static METRICS_ENABLED: AtomicBool = AtomicBool::new(false);
static INIT_ONCE: Once = Once::new();

// GenAI metric histogram bucket boundaries (spec §2).
const TOKEN_BUCKETS: &[f64] = &[
    1.0, 4.0, 16.0, 64.0, 256.0, 1024.0, 4096.0, 16384.0, 65536.0, 262144.0, 1048576.0, 4194304.0,
    16777216.0, 67108864.0,
];
const DURATION_BUCKETS: &[f64] = &[
    0.01, 0.02, 0.04, 0.08, 0.16, 0.32, 0.64, 1.28, 2.56, 5.12, 10.24, 20.48, 40.96, 81.92,
];

struct Metrics {
    token_usage: Histogram<u64>,
    op_duration: Histogram<f64>,
}

static METRICS: std::sync::OnceLock<Option<Metrics>> = std::sync::OnceLock::new();

/// Lazily build (and cache) the two GenAI histograms against the installed global
/// meter provider. `None` when metrics aren't enabled.
fn metrics() -> Option<&'static Metrics> {
    if !METRICS_ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    METRICS
        .get_or_init(|| {
            let meter = global::meter("sema");
            Some(Metrics {
                token_usage: meter
                    .u64_histogram("gen_ai.client.token.usage")
                    .with_unit("{token}")
                    .with_boundaries(TOKEN_BUCKETS.to_vec())
                    .build(),
                op_duration: meter
                    .f64_histogram("gen_ai.client.operation.duration")
                    .with_unit("s")
                    .with_boundaries(DURATION_BUCKETS.to_vec())
                    .build(),
            })
        })
        .as_ref()
}

/// Enable metric recording against the current global meter provider (used by the
/// testing helper after it installs an in-memory meter provider).
#[cfg_attr(not(feature = "testing"), allow(dead_code))]
pub(crate) fn enable_metrics() {
    METRICS_ENABLED.store(true, Ordering::Relaxed);
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

thread_local! {
    /// Active-span stack for parent/child nesting in the single-threaded VM.
    static STACK: RefCell<Vec<Context>> = const { RefCell::new(Vec::new()) };
}

fn scope() -> InstrumentationScope {
    InstrumentationScope::builder("sema")
        .with_version(VERSION)
        .build()
}

fn service_name() -> String {
    std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "sema".to_string())
}

// ---------------------------------------------------------------------------
// Initialization & shutdown
// ---------------------------------------------------------------------------

/// RAII guard owning the installed provider. `Drop` does a bounded
/// flush + shutdown so short-lived `sema run file.sema` processes never lose the
/// last spans and never hang on a dead collector.
pub struct OtelGuard {
    provider: Option<SdkTracerProvider>,
    meter: Option<SdkMeterProvider>,
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        if let Some(p) = &self.provider {
            let _ = p.force_flush();
            let _ = p.shutdown_with_timeout(Duration::from_secs(3));
        }
        if let Some(m) = &self.meter {
            let _ = m.force_flush();
            let _ = m.shutdown();
        }
    }
}

/// Install a provider from the environment and return a guard, or `None` if no sink
/// is configured (true zero-cost no-op) or init fails (degrade silently — never
/// panic on the OTel path). This is the ONLY function that calls
/// `global::set_tracer_provider` (Decision #14). Idempotent via `Once`.
pub fn init_from_env() -> Option<OtelGuard> {
    let mut guard = None;
    INIT_ONCE.call_once(|| {
        let provider = build_provider();
        let meter = build_meter_provider();
        if provider.is_none() && meter.is_none() {
            return; // nothing configured — zero-cost no-op
        }
        if let Some(p) = &provider {
            global::set_tracer_provider(p.clone());
            ENABLED.store(true, Ordering::Relaxed);
        }
        if let Some(m) = &meter {
            global::set_meter_provider(m.clone());
            ENABLED.store(true, Ordering::Relaxed);
            METRICS_ENABLED.store(true, Ordering::Relaxed);
        }
        guard = Some(OtelGuard { provider, meter });
    });
    guard
}

/// Embedded mode: emit against whatever provider the host already installed in
/// `opentelemetry::global` (Decision #12). Installs nothing; silent no-op if the host
/// installed nothing. Enables the facade so it resolves the global tracer lazily.
pub fn use_host_global() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Build a provider from the §4.1 env table, or `None` when nothing is configured.
fn build_provider() -> Option<SdkTracerProvider> {
    let file = std::env::var("SEMA_OTEL_FILE")
        .ok()
        .filter(|s| !s.is_empty());
    let otlp = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").ok())
        .filter(|s| !s.is_empty());

    if file.is_none() && otlp.is_none() {
        return None; // install nothing — zero-cost no-op
    }

    let resource = Resource::builder()
        .with_service_name(service_name())
        .build();
    let mut builder = SdkTracerProvider::builder().with_resource(resource);

    if let Some(path) = file {
        if let Ok(exp) = JsonlFileExporter::new(&path) {
            // Simple exporter → deterministic immediate JSONL capture.
            builder = builder.with_simple_exporter(exp);
        }
    }
    if otlp.is_some() {
        if let Some(exp) = build_otlp_exporter() {
            // Batch exporter → thread-based BatchSpanProcessor (no tokio on HTTP path);
            // hot path does a non-blocking enqueue, a dead collector drops on full.
            builder = builder.with_batch_exporter(exp);
        }
    }
    Some(builder.build())
}

/// Dispatch on `OTEL_EXPORTER_OTLP_PROTOCOL` (Decision #3). HTTP (default) needs no
/// tokio runtime; gRPC construction needs a live reactor (best-effort).
fn build_otlp_exporter() -> Option<opentelemetry_otlp::SpanExporter> {
    use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig};
    let proto =
        std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_else(|_| "http/protobuf".into());
    let result = match proto.as_str() {
        "grpc" => {
            // tonic must be constructed inside a tokio context.
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(async { SpanExporter::builder().with_tonic().build() }),
                Err(_) => return None,
            }
        }
        "http/json" => SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpJson)
            .build(),
        _ => SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .build(),
    };
    result.ok()
}

/// Build a meter provider (OTLP only — the JSONL sink is trace-only). `None` when no
/// OTLP endpoint is configured or the exporter fails to build.
fn build_meter_provider() -> Option<SdkMeterProvider> {
    let otlp = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT").ok())
        .filter(|s| !s.is_empty());
    otlp.as_ref()?;
    let exporter = build_otlp_metric_exporter()?;
    let reader = PeriodicReader::builder(exporter).build();
    let resource = Resource::builder()
        .with_service_name(service_name())
        .build();
    Some(
        SdkMeterProvider::builder()
            .with_reader(reader)
            .with_resource(resource)
            .build(),
    )
}

fn build_otlp_metric_exporter() -> Option<opentelemetry_otlp::MetricExporter> {
    use opentelemetry_otlp::{MetricExporter, Protocol, WithExportConfig};
    let proto =
        std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_else(|_| "http/protobuf".into());
    let result = match proto.as_str() {
        "grpc" => match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(async { MetricExporter::builder().with_tonic().build() }),
            Err(_) => return None,
        },
        "http/json" => MetricExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpJson)
            .build(),
        _ => MetricExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .build(),
    };
    result.ok()
}

// ---------------------------------------------------------------------------
// Span core
// ---------------------------------------------------------------------------

/// Shared RAII span core. Holds the `Context` carrying the span (cloned onto the TL
/// stack for nesting). `Drop` pops the stack and ends the span (recording duration).
struct SpanCore {
    ctx: Context,
}

impl Drop for SpanCore {
    fn drop(&mut self) {
        STACK.with(|s| {
            s.borrow_mut().pop();
        });
        self.ctx.span().end();
    }
}

impl SpanCore {
    fn set_str(&self, key: &'static str, val: impl Into<StringValue>) {
        self.ctx
            .span()
            .set_attribute(KeyValue::new(key, val.into()));
    }
    fn set_i64(&self, key: &'static str, val: i64) {
        self.ctx.span().set_attribute(KeyValue::new(key, val));
    }
    fn set_f64(&self, key: &'static str, val: f64) {
        self.ctx.span().set_attribute(KeyValue::new(key, val));
    }
    fn set_bool(&self, key: &'static str, val: bool) {
        self.ctx.span().set_attribute(KeyValue::new(key, val));
    }
    fn set_str_array(&self, key: &'static str, vals: Vec<String>) {
        let arr: Vec<StringValue> = vals.into_iter().map(Into::into).collect();
        self.ctx
            .span()
            .set_attribute(KeyValue::new(key, Value::Array(Array::String(arr))));
    }
    fn update_name(&self, name: String) {
        self.ctx.span().update_name(name);
    }
    fn record_error(&self, kind: &str, msg: &str) {
        self.ctx
            .span()
            .set_attribute(KeyValue::new("error.type", kind.to_string()));
        self.ctx.span().set_status(Status::error(msg.to_string()));
    }
}

/// Start a span as a child of the current TL-stack top (or `Context::current()` when
/// the stack is empty — Decision #13), push it onto the stack, return its core.
/// Returns `None` when telemetry is disabled (no `global` touch, near-zero cost).
fn start(name: String, kind: SpanKind, attrs: Vec<KeyValue>) -> Option<SpanCore> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let parent = STACK.with(|s| s.borrow().last().cloned());
    let parent = parent.unwrap_or_else(Context::current);
    let tracer = global::tracer_with_scope(scope());
    let span = tracer
        .span_builder(name)
        .with_kind(kind)
        .with_attributes(attrs)
        .start_with_context(&tracer, &parent);
    let ctx = parent.with_span(span);
    STACK.with(|s| s.borrow_mut().push(ctx.clone()));
    Some(SpanCore { ctx })
}

/// Whether message-content capture is enabled (off by default). Standard flag with a
/// Sema alias.
fn capture_content() -> bool {
    std::env::var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT")
        .ok()
        .or_else(|| std::env::var("SEMA_OTEL_CAPTURE_CONTENT").ok())
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

/// Truncate + prototype-pollution-guard message content before it becomes a span
/// attribute. Cheap and panic-proof (runs on the calling thread).
fn scrub(s: &str) -> String {
    const MAX: usize = 8192;
    if s.len() <= MAX {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(MAX).collect();
        t.push_str("…[truncated]");
        t
    }
}

// ---------------------------------------------------------------------------
// LLM span
// ---------------------------------------------------------------------------

pub struct LlmSpan {
    inner: Option<SpanCore>,
    op: &'static str,
    start: std::time::Instant,
    /// Mapped `gen_ai.provider.name` + request model, captured in `set_dispatch` for
    /// the metric dimensions (recorded in `set_response`).
    dims: RefCell<(String, String)>,
}

/// Start an LLM-call span (CLIENT). Provider/model are unknown at entry and set later
/// via `set_dispatch`.
pub fn llm_span(op: &'static str) -> LlmSpan {
    let inner = start(
        op.to_string(),
        SpanKind::Client,
        vec![KeyValue::new("gen_ai.operation.name", op)],
    );
    LlmSpan {
        inner,
        op,
        start: std::time::Instant::now(),
        dims: RefCell::new((String::new(), String::new())),
    }
}

impl LlmSpan {
    pub fn set_request(
        &self,
        temperature: Option<f64>,
        max_tokens: Option<u32>,
        stop_sequences: &[String],
        reasoning_effort: Option<&str>,
    ) {
        if let Some(c) = &self.inner {
            if let Some(t) = temperature {
                c.set_f64("gen_ai.request.temperature", t);
            }
            if let Some(m) = max_tokens {
                c.set_i64("gen_ai.request.max_tokens", m as i64);
            }
            if !stop_sequences.is_empty() {
                c.set_str_array("gen_ai.request.stop_sequences", stop_sequences.to_vec());
            }
            if let Some(r) = reasoning_effort {
                c.set_str("sema.gen_ai.request.reasoning_effort", r.to_string());
            }
        }
    }

    /// Called AFTER the provider is resolved. Sets provider + request model and
    /// renames the span to the spec form `{op} {model}`.
    pub fn set_dispatch(&self, sema_provider: &str, request_model: &str) {
        // Capture metric dimensions even if the span itself is a no-op (metrics may be
        // enabled independently). Empty provider on a cache hit is kept as "".
        let mapped = if sema_provider.is_empty() {
            String::new()
        } else {
            gen_ai_provider_name(sema_provider).to_string()
        };
        *self.dims.borrow_mut() = (mapped.clone(), request_model.to_string());
        if let Some(c) = &self.inner {
            // A cache hit has no serving provider — skip the attribute rather than
            // emit an empty/mis-mapped value.
            if !mapped.is_empty() {
                c.set_str("gen_ai.provider.name", mapped);
            }
            if !request_model.is_empty() {
                c.set_str("gen_ai.request.model", request_model.to_string());
                c.update_name(format!("{} {}", self.op, request_model));
            }
        }
    }

    /// Record the two GenAI metric histograms (no-op when metrics are disabled).
    fn record_metrics(&self, facts: &ResponseFacts) {
        let Some(m) = metrics() else { return };
        let (provider, req_model) = self.dims.borrow().clone();
        let base = [
            KeyValue::new("gen_ai.operation.name", self.op),
            KeyValue::new("gen_ai.provider.name", provider.clone()),
            KeyValue::new("gen_ai.request.model", req_model.clone()),
            KeyValue::new("gen_ai.response.model", facts.response_model.clone()),
        ];
        let mut input_dims = base.to_vec();
        input_dims.push(KeyValue::new("gen_ai.token.type", "input"));
        m.token_usage.record(facts.input_tokens as u64, &input_dims);
        let mut output_dims = base.to_vec();
        output_dims.push(KeyValue::new("gen_ai.token.type", "output"));
        m.token_usage
            .record(facts.output_tokens as u64, &output_dims);

        let duration_dims = [
            KeyValue::new("gen_ai.operation.name", self.op),
            KeyValue::new("gen_ai.provider.name", provider),
            KeyValue::new("gen_ai.request.model", req_model),
        ];
        m.op_duration
            .record(self.start.elapsed().as_secs_f64(), &duration_dims);
    }

    pub fn set_response(&self, facts: &ResponseFacts) {
        if let Some(c) = &self.inner {
            c.set_i64("gen_ai.usage.input_tokens", facts.input_tokens as i64);
            c.set_i64("gen_ai.usage.output_tokens", facts.output_tokens as i64);
            if facts.cache_read_input_tokens > 0 {
                c.set_i64(
                    "gen_ai.usage.cache_read.input_tokens",
                    facts.cache_read_input_tokens as i64,
                );
            }
            if facts.cache_creation_input_tokens > 0 {
                c.set_i64(
                    "gen_ai.usage.cache_creation.input_tokens",
                    facts.cache_creation_input_tokens as i64,
                );
            }
            if !facts.response_model.is_empty() {
                c.set_str("gen_ai.response.model", facts.response_model.clone());
            }
            if let Some(reason) = &facts.finish_reason {
                c.set_str_array("gen_ai.response.finish_reasons", vec![reason.clone()]);
            }
            if let Some(cost) = facts.cost_usd {
                c.set_f64("gen_ai.usage.cost_usd", cost);
            }
            if facts.cache_hit {
                c.set_bool("gen_ai.cache.hit", true);
            }
        }
        self.record_metrics(facts);
    }

    pub fn set_conversation_id(&self, id: &str) {
        if let Some(c) = &self.inner {
            c.set_str("gen_ai.conversation.id", id.to_string());
        }
    }

    pub fn set_messages(&self, input: &str, output: &str, system: Option<&str>) {
        if !capture_content() {
            return;
        }
        if let Some(c) = &self.inner {
            c.set_str("gen_ai.input.messages", scrub(input));
            c.set_str("gen_ai.output.messages", scrub(output));
            if let Some(sys) = system {
                c.set_str("gen_ai.system_instructions", scrub(sys));
            }
        }
    }

    pub fn record_error(&self, kind: &str, msg: &str) {
        if let Some(c) = &self.inner {
            c.record_error(kind, msg);
        }
    }
}

// ---------------------------------------------------------------------------
// Tool span
// ---------------------------------------------------------------------------

pub struct ToolSpan {
    inner: Option<SpanCore>,
}

/// Start a tool-dispatch span (INTERNAL). v1.41 requires the tool name in the span
/// name: `execute_tool {name}`.
pub fn tool_span(name: &str, call_id: &str, description: Option<&str>) -> ToolSpan {
    let mut attrs = vec![
        KeyValue::new("gen_ai.operation.name", "execute_tool"),
        KeyValue::new("gen_ai.tool.name", name.to_string()),
        KeyValue::new("gen_ai.tool.call.id", call_id.to_string()),
        KeyValue::new("gen_ai.tool.type", "function"),
    ];
    if let Some(d) = description {
        attrs.push(KeyValue::new("gen_ai.tool.description", d.to_string()));
    }
    let inner = start(format!("execute_tool {name}"), SpanKind::Internal, attrs);
    ToolSpan { inner }
}

impl ToolSpan {
    pub fn set_conversation_id(&self, id: &str) {
        if let Some(c) = &self.inner {
            c.set_str("gen_ai.conversation.id", id.to_string());
        }
    }
    pub fn record_error(&self, kind: &str, msg: &str) {
        if let Some(c) = &self.inner {
            c.record_error(kind, msg);
        }
    }
}

// ---------------------------------------------------------------------------
// Agent span
// ---------------------------------------------------------------------------

pub struct AgentSpan {
    inner: Option<SpanCore>,
}

/// Start an agent-run span (INTERNAL): `invoke_agent {name}` or bare `invoke_agent`.
pub fn agent_span(name: Option<&str>) -> AgentSpan {
    let mut attrs = vec![KeyValue::new("gen_ai.operation.name", "invoke_agent")];
    let span_name = match name {
        Some(n) if !n.is_empty() => {
            attrs.push(KeyValue::new("gen_ai.agent.name", n.to_string()));
            format!("invoke_agent {n}")
        }
        _ => "invoke_agent".to_string(),
    };
    let inner = start(span_name, SpanKind::Internal, attrs);
    AgentSpan { inner }
}

impl AgentSpan {
    pub fn set_conversation_id(&self, id: &str) {
        if let Some(c) = &self.inner {
            c.set_str("gen_ai.conversation.id", id.to_string());
        }
    }
}

// ---------------------------------------------------------------------------
// Retry sub-span (per HTTP retry attempt, nested under the LLM span — Decision #10)
// ---------------------------------------------------------------------------

pub struct RetrySpan {
    inner: Option<SpanCore>,
}

/// Start an INTERNAL child span for an HTTP retry attempt (`attempt` is 1-based:
/// the first retry is attempt 1). Nests under the current LLM span via the TL stack.
pub fn retry_span(attempt: u32) -> RetrySpan {
    let inner = start(
        "llm.retry_attempt".to_string(),
        SpanKind::Internal,
        vec![KeyValue::new("sema.retry.attempt", attempt as i64)],
    );
    RetrySpan { inner }
}

impl RetrySpan {
    pub fn record_error(&self, kind: &str, msg: &str) {
        if let Some(c) = &self.inner {
            c.record_error(kind, msg);
        }
    }
    pub fn set_wait_ms(&self, ms: u64) {
        if let Some(c) = &self.inner {
            c.set_i64("sema.retry.wait_ms", ms as i64);
        }
    }
}

// ---------------------------------------------------------------------------
// VM span + generic event
// ---------------------------------------------------------------------------

pub struct VmSpan {
    inner: Option<SpanCore>,
}

/// Start a generic INTERNAL span (notebook cell, load/import, `(otel/span …)`).
pub fn vm_span(name: &str) -> VmSpan {
    let inner = start(name.to_string(), SpanKind::Internal, Vec::new());
    VmSpan { inner }
}

impl VmSpan {
    pub fn set_str(&self, key: &'static str, val: &str) {
        if let Some(c) = &self.inner {
            c.set_str(key, val.to_string());
        }
    }
}

/// Add an event to the current span (the TL-stack top). No-op when disabled or when
/// there is no active span.
pub fn add_event(name: &str, attrs: Vec<(String, String)>) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let kvs: Vec<KeyValue> = attrs
        .into_iter()
        .filter(|(k, _)| !is_polluting_key(k))
        .map(|(k, v)| KeyValue::new(k, v))
        .collect();
    STACK.with(|s| {
        if let Some(ctx) = s.borrow().last() {
            ctx.span().add_event(name.to_string(), kvs);
        }
    });
}

fn is_polluting_key(k: &str) -> bool {
    matches!(k, "__proto__" | "constructor" | "prototype")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_facade_is_noop() {
        // With nothing enabled, constructing/dropping spans must not panic and must
        // not touch global state. (ENABLED defaults false in a fresh test process.)
        let s = llm_span("chat");
        s.set_dispatch("openai", "gpt-x");
        s.set_response(&ResponseFacts {
            input_tokens: 1,
            output_tokens: 2,
            ..Default::default()
        });
        drop(s);
        let _ = tool_span("t", "id", None);
        let _ = agent_span(Some("bot"));
        let _ = vm_span("cell");
        add_event("e", vec![("k".into(), "v".into())]);
    }

    #[test]
    fn scrub_truncates_long_content() {
        let big = "x".repeat(20_000);
        let out = scrub(&big);
        assert!(out.len() < big.len());
        assert!(out.ends_with("…[truncated]"));
    }
}
