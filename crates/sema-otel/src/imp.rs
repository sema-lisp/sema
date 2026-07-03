//! Native (non-wasm) OpenTelemetry implementation of the facade.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Once, OnceLock};
use std::time::Duration;

use opentelemetry::global;
use opentelemetry::metrics::Histogram;
use opentelemetry::trace::{Span, SpanKind, Status, TraceContextExt, Tracer, TracerProvider};
use opentelemetry::{Array, Context, InstrumentationScope, KeyValue, StringValue, Value};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;

use crate::compat;
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
/// Set true once a provider is actually installed (by `init_from_env` or `configure`).
/// A no-op init (nothing configured) leaves it `false`, so a later programmatic
/// `otel/configure` can still install exactly one provider per process.
static PROVIDER_INSTALLED: AtomicBool = AtomicBool::new(false);

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
    /// Current GenAI conversation / session / user identity, applied to every span
    /// started while a scope is active (set by agent runs + standalone completions).
    static CONVERSATION_ID: RefCell<Option<String>> = const { RefCell::new(None) };
    static SESSION_ID: RefCell<Option<String>> = const { RefCell::new(None) };
    static USER_ID: RefCell<Option<String>> = const { RefCell::new(None) };
    static CONV_COUNTER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Host-supplied tracer provider (embedded `OwnProvider` mode). When set, spans are
    /// emitted against it instead of the global provider, WITHOUT installing a global.
    /// Thread-local because the eval path is `!Send`/`Rc`-based and single-threaded.
    static OWNED_PROVIDER: RefCell<Option<SdkTracerProvider>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------------------
// Per-task OTel context swap (cooperative-scheduler isolation)
// ---------------------------------------------------------------------------

/// A snapshot of the per-thread otel context owned by one scheduler task: the
/// active-span stack plus the conversation/session/user identity. The scheduler
/// swaps one of these into the thread-locals on task entry and takes it back out
/// on task leave, so a task that parks mid-span (its `SpanCore` guard still on
/// the stack) cannot corrupt a sibling task's stack or ids.
#[derive(Default)]
pub struct OtelTaskCtx {
    stack: Vec<Context>,
    conversation_id: Option<String>,
    session_id: Option<String>,
    user_id: Option<String>,
}

/// Move the current thread's otel context out of the thread-locals (leaving them
/// empty), returning it so the scheduler can store it on the parked task.
pub fn take_task_otel() -> OtelTaskCtx {
    OtelTaskCtx {
        stack: STACK.with(|s| std::mem::take(&mut *s.borrow_mut())),
        conversation_id: CONVERSATION_ID.with(|c| c.borrow_mut().take()),
        session_id: SESSION_ID.with(|c| c.borrow_mut().take()),
        user_id: USER_ID.with(|c| c.borrow_mut().take()),
    }
}

/// Install `ctx` into the thread-locals, returning the context it displaced (so
/// the scheduler can restore the previous task's context after this task's step).
pub fn install_task_otel(ctx: OtelTaskCtx) -> OtelTaskCtx {
    OtelTaskCtx {
        stack: STACK.with(|s| std::mem::replace(&mut *s.borrow_mut(), ctx.stack)),
        conversation_id: CONVERSATION_ID
            .with(|c| std::mem::replace(&mut *c.borrow_mut(), ctx.conversation_id)),
        session_id: SESSION_ID.with(|c| std::mem::replace(&mut *c.borrow_mut(), ctx.session_id)),
        user_id: USER_ID.with(|c| std::mem::replace(&mut *c.borrow_mut(), ctx.user_id)),
    }
}

/// Capture the spawning context to seed onto a newly-spawned task: its
/// conversation/session/user identity, plus the spawner's CURRENT active span (only
/// the top-of-stack `Context`, if any) as the child's parent. So spans opened in the
/// child task nest under the spawner and share its trace — proper distributed-trace
/// propagation across `async/spawn` (a `(with-span … (async/map …))` becomes one
/// connected tree, not N disconnected single-span traces).
///
/// We copy ONLY the top context — one parent marker — never the whole stack: the
/// child pushes/pops its OWN spans above this seed and never pops the seed itself, so
/// the spawner's in-flight stack is never mis-popped (the hazard the empty-stack seed
/// originally guarded against). A top-level spawn (empty stack) seeds nothing → the
/// child's spans are trace roots, exactly as before — so sibling top-level tasks stay
/// in distinct traces (per-task isolation preserved).
pub fn current_conversation_scope() -> OtelTaskCtx {
    let parent = STACK.with(|s| s.borrow().last().cloned());
    OtelTaskCtx {
        stack: parent.into_iter().collect(),
        conversation_id: CONVERSATION_ID.with(|c| c.borrow().clone()),
        session_id: SESSION_ID.with(|c| c.borrow().clone()),
        user_id: USER_ID.with(|c| c.borrow().clone()),
    }
}

/// Register the type-erased per-task otel callbacks with `sema-core` so the
/// scheduler (in `sema-vm`, which must not depend on `sema-otel`) can swap the
/// otel context on task-switch. Called once at interpreter startup. The
/// `Box<dyn Any>` carries an [`OtelTaskCtx`]; a non-`OtelTaskCtx` box (e.g. the
/// `()` placeholder when no context was ever captured) installs the default.
pub fn register_task_callbacks() {
    fn take() -> Box<dyn std::any::Any> {
        Box::new(take_task_otel())
    }
    fn install(ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
        let ctx = ctx
            .downcast::<OtelTaskCtx>()
            .map(|b| *b)
            .unwrap_or_default();
        Box::new(install_task_otel(ctx))
    }
    fn scope_ctx() -> Box<dyn std::any::Any> {
        Box::new(current_conversation_scope())
    }
    sema_core::set_otel_task_callbacks(take, install, scope_ctx);
}

/// How an embedded host wires Sema's telemetry (Decisions #12, #14, #16). The
/// standalone CLI uses `init_from_env()` directly; embedders pass one of these to
/// `InterpreterBuilder::with_telemetry`.
pub enum TelemetryMode {
    /// Default: emit nothing, touch no global state (pure no-op).
    Off,
    /// Emit against whatever provider the host already installed in `global` (silent
    /// no-op if none).
    UseHostGlobal,
    /// Emit against a provider the host hands us, installing NO global.
    OwnProvider(SdkTracerProvider),
    /// Self-install from the environment (standalone behavior) — but DECLINE and defer
    /// to the host if a real global provider is already installed (Decision #16). The
    /// returned `OtelGuard` is handed back to the host to own.
    FromEnv,
}

/// Emit subsequent spans against a host-supplied provider (no global install).
pub fn use_provider(provider: SdkTracerProvider) {
    OWNED_PROVIDER.with(|c| *c.borrow_mut() = Some(provider));
    ENABLED.store(true, Ordering::Relaxed);
}

/// Activate a telemetry mode. Returns an `OtelGuard` ONLY for `FromEnv` when Sema
/// self-installs; all other modes install nothing and return `None`. This is the
/// single activation entry point for embedders.
///
/// Precedence (Decision #16) is **explicit**, not auto-detected: opentelemetry 0.32
/// exposes no way to detect an externally-installed provider without emitting a probe
/// span into the host's pipeline, so `FromEnv` self-installs unconditionally. An
/// embedder that already runs OTel should use `UseHostGlobal` or `OwnProvider` (which
/// install nothing) instead of `FromEnv`.
pub fn activate(mode: TelemetryMode) -> Option<OtelGuard> {
    match mode {
        // `Off` is a non-configuring no-op: it leaves any already-installed telemetry
        // (e.g. the standalone CLI's `init_from_env`) untouched and emits nothing extra.
        TelemetryMode::Off => None,
        TelemetryMode::UseHostGlobal => {
            // Clear any owned provider from a prior mode so we resolve the global.
            OWNED_PROVIDER.with(|c| *c.borrow_mut() = None);
            use_host_global();
            None
        }
        TelemetryMode::OwnProvider(p) => {
            use_provider(p);
            None
        }
        TelemetryMode::FromEnv => {
            OWNED_PROVIDER.with(|c| *c.borrow_mut() = None);
            init_from_env()
        }
    }
}

/// RAII scope for the conversation/session/user identity. Restores the PREVIOUS
/// values on drop so a standalone completion nested inside an agent run cannot wipe
/// the agent's id.
pub struct ConversationGuard {
    prev_conv: Option<String>,
    prev_session: Option<String>,
    prev_user: Option<String>,
}

impl Drop for ConversationGuard {
    fn drop(&mut self) {
        CONVERSATION_ID.with(|c| *c.borrow_mut() = self.prev_conv.take());
        SESSION_ID.with(|c| *c.borrow_mut() = self.prev_session.take());
        USER_ID.with(|c| *c.borrow_mut() = self.prev_user.take());
    }
}

/// Open a conversation scope. `session_id` defaults to the conversation id (so a single
/// run groups in session-aware backends like Langfuse); `user_id` is inherited when
/// `None`. Every span started while the returned guard is alive carries
/// `gen_ai.conversation.id` (+ `session.id` / `user.id` for Langfuse).
pub fn set_conversation_scope(
    conversation_id: &str,
    session_id: Option<&str>,
    user_id: Option<&str>,
) -> ConversationGuard {
    let prev_conv = CONVERSATION_ID.with(|c| c.borrow_mut().replace(conversation_id.to_string()));
    let session = session_id
        .map(str::to_string)
        .unwrap_or_else(|| conversation_id.to_string());
    let prev_session = SESSION_ID.with(|c| c.borrow_mut().replace(session));
    let prev_user = USER_ID.with(|c| {
        let mut b = c.borrow_mut();
        let prev = b.clone();
        if let Some(u) = user_id {
            *b = Some(u.to_string());
        }
        prev
    });
    ConversationGuard {
        prev_conv,
        prev_session,
        prev_user,
    }
}

/// The conversation id of the active scope, if any.
pub fn current_conversation_id() -> Option<String> {
    CONVERSATION_ID.with(|c| c.borrow().clone())
}

/// Generate a fresh, cheap conversation id (monotonic counter mixed with wall-clock).
pub fn new_conversation_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let c = CONV_COUNTER.with(|c| {
        let v = c.get().wrapping_add(1);
        c.set(v);
        v
    });
    format!(
        "conv_{:016x}",
        nanos ^ c.wrapping_mul(0x9E37_79B9_7F4A_7C15)
    )
}

/// OTel schema version our `gen_ai.*` attributes conform to (GenAI semconv baseline).
const OTEL_SCHEMA_URL: &str = "https://opentelemetry.io/schemas/1.37.0";

fn scope() -> InstrumentationScope {
    // The instrumentation scope identifies *this* library and the semconv schema it
    // emits, plus a couple of descriptive attributes so the scope isn't bare.
    InstrumentationScope::builder("sema")
        .with_version(VERSION)
        .with_schema_url(OTEL_SCHEMA_URL)
        .with_attributes([
            KeyValue::new("sema.otel.instrumentation", "gen_ai"),
            KeyValue::new("telemetry.distro.name", "sema-otel"),
            KeyValue::new("telemetry.distro.version", VERSION),
        ])
        .build()
}

fn service_name() -> String {
    std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "sema".to_string())
}

/// Build the OTel `Resource` describing the producing service. Enriched beyond the
/// bare service name so backends can filter/group on version + language + runtime +
/// environment.
fn build_resource() -> Resource {
    let mut attrs = vec![
        KeyValue::new("service.version", VERSION),
        KeyValue::new("telemetry.sdk.language", "rust"),
        KeyValue::new("process.runtime.name", "sema"),
        KeyValue::new("process.runtime.version", VERSION),
    ];
    // Standard env-name attribute (Langfuse + Logfire filter on it). Zero program change.
    if let Some(env) = std::env::var("SEMA_OTEL_ENVIRONMENT")
        .ok()
        .or_else(|| std::env::var("DEPLOYMENT_ENVIRONMENT").ok())
        .filter(|s| !s.is_empty())
    {
        attrs.push(KeyValue::new("deployment.environment.name", env));
    }
    Resource::builder()
        .with_service_name(service_name())
        .with_schema_url(attrs, OTEL_SCHEMA_URL)
        .build()
}

// ---------------------------------------------------------------------------
// Initialization & shutdown
// ---------------------------------------------------------------------------

struct ShutdownState {
    provider: Option<SdkTracerProvider>,
    meter: Option<SdkMeterProvider>,
}

/// The installed providers awaiting flush. Drained exactly once — by whichever of
/// `OtelGuard::drop` (normal return) or the `atexit` hook (a `std::process::exit`
/// path) fires first.
static SHUTDOWN: std::sync::Mutex<Option<ShutdownState>> = std::sync::Mutex::new(None);
static ATEXIT_ONCE: Once = Once::new();

/// Register the providers for flush-at-exit and install the `atexit` hook once. The
/// hook guarantees OTLP spans flush even on the CLI's many `std::process::exit` paths
/// (which skip `Drop`); `libc::exit` runs C `atexit` handlers.
fn register_shutdown(provider: Option<SdkTracerProvider>, meter: Option<SdkMeterProvider>) {
    if let Ok(mut g) = SHUTDOWN.lock() {
        *g = Some(ShutdownState { provider, meter });
    }
    ATEXIT_ONCE.call_once(|| unsafe {
        libc::atexit(atexit_flush);
    });
}

extern "C" fn atexit_flush() {
    take_and_flush();
}

/// Take the pending providers (if any) and run a bounded flush + shutdown. The
/// async-runtime (gRPC) processors' force_flush/shutdown block until export completes
/// and IGNORE their timeout — against a dead collector that could hang exit. So run
/// the work on a side thread joined with a hard wall-clock budget; if it overruns,
/// abandon it and let process teardown reclaim the static runtime. (The JSONL simple
/// exporter is synchronous and flushed inside this same call — unaffected.) Idempotent.
fn take_and_flush() {
    let state = SHUTDOWN.lock().ok().and_then(|mut g| g.take());
    let Some(state) = state else { return };
    let handle = std::thread::spawn(move || {
        if let Some(p) = state.provider {
            let _ = p.force_flush();
            let _ = p.shutdown_with_timeout(Duration::from_secs(3));
        }
        if let Some(m) = state.meter {
            let _ = m.force_flush();
            let _ = m.shutdown();
        }
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    while !handle.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    if handle.is_finished() {
        let _ = handle.join();
    }
    // else: abandon the thread; process exit tears down the runtime.
}

/// RAII guard for a Sema-installed provider. `Drop` triggers the bounded flush on
/// normal return; the `atexit` hook covers `std::process::exit` paths. Either way the
/// flush runs exactly once.
pub struct OtelGuard {
    _private: (),
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        take_and_flush();
    }
}

/// Install a provider from the environment and return a guard, or `None` if no sink
/// is configured (true zero-cost no-op) or init fails (degrade silently — never
/// panic on the OTel path). This is the ONLY function that calls
/// `global::set_tracer_provider` (Decision #14). Reads the §4.1 env table; the
/// programmatic path (`configure`) writes that same env before calling in.
pub fn init_from_env() -> Option<OtelGuard> {
    install_provider()
}

/// Build + install the tracer/meter providers from the current env, guarded so at most
/// one provider is installed per process. A no-op call (nothing configured) does NOT
/// consume the guard, so the CLI's startup `init_from_env` can be a no-op yet still
/// leave a later `otel/configure` free to install. Shared by `init_from_env` +
/// `configure`.
fn install_provider() -> Option<OtelGuard> {
    // Already installed (by a prior env init or configure)? The global provider can only
    // be set once, so bail without touching it.
    if PROVIDER_INSTALLED.load(Ordering::SeqCst) {
        return None;
    }
    let provider = build_provider();
    let meter = build_meter_provider();
    if provider.is_none() && meter.is_none() {
        return None; // nothing configured — zero-cost no-op, guard left unset
    }
    // Claim the single install slot. A racing second caller sees `true` and bails.
    if PROVIDER_INSTALLED.swap(true, Ordering::SeqCst) {
        return None;
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
    // Register for flush-at-exit (Drop + atexit, take-once) so spans survive both
    // normal return AND std::process::exit.
    register_shutdown(provider, meter);
    Some(OtelGuard { _private: () })
}

/// Programmatic OTel configuration produced by `otel/configure`, so a Sema script can
/// point itself at a backend without any environment variables. Each field mirrors the
/// env var of the same role (`endpoint` → `OTEL_EXPORTER_OTLP_ENDPOINT`, etc.);
/// `configure` writes them into the process env and then runs the ordinary env-driven
/// install path, so the exporter build + every hot-path reader (service name, release,
/// content capture) pick them up unchanged. Keeps `sema-otel`'s public surface free of
/// `opentelemetry` types.
#[derive(Debug, Clone, Default)]
pub struct OtelConfig {
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` — the backend address; setting it turns tracing on.
    pub endpoint: Option<String>,
    /// `SEMA_OTEL_FILE` — write JSONL spans to this path instead of the network.
    pub file: Option<String>,
    /// `OTEL_EXPORTER_OTLP_PROTOCOL` — `http/protobuf` (default) · `http/json` · `grpc`.
    pub protocol: Option<String>,
    /// `OTEL_EXPORTER_OTLP_HEADERS` — already formatted as comma-separated `name=value`
    /// pairs (auth etc.). The builtin joins a header map / `:key` shorthand into this.
    pub headers: Option<String>,
    /// `OTEL_SERVICE_NAME` — the name runs appear under in the backend.
    pub service_name: Option<String>,
    /// `SEMA_OTEL_ENVIRONMENT` — deployment env label (`prod`, `staging`, …).
    pub environment: Option<String>,
    /// `SEMA_OTEL_RELEASE` — release/version stamp.
    pub release: Option<String>,
    /// `SEMA_OTEL_CAPTURE_CONTENT` — record prompt/response text (off by default).
    pub capture_content: Option<bool>,
}

impl OtelConfig {
    /// Write each set field into the process environment so the env-driven build path and
    /// the hot-path readers observe it. Called just before install, before any exporter is
    /// built, on the (single-threaded) VM thread.
    fn apply_to_env(&self) {
        fn set(key: &str, val: &Option<String>) {
            if let Some(v) = val.as_deref().filter(|s| !s.is_empty()) {
                // SAFETY: called on the single VM thread before any exporter/reader runs.
                unsafe { std::env::set_var(key, v) };
            }
        }
        set("OTEL_EXPORTER_OTLP_ENDPOINT", &self.endpoint);
        set("SEMA_OTEL_FILE", &self.file);
        set("OTEL_EXPORTER_OTLP_PROTOCOL", &self.protocol);
        set("OTEL_EXPORTER_OTLP_HEADERS", &self.headers);
        set("OTEL_SERVICE_NAME", &self.service_name);
        set("SEMA_OTEL_ENVIRONMENT", &self.environment);
        set("SEMA_OTEL_RELEASE", &self.release);
        if let Some(c) = self.capture_content {
            // SAFETY: as above.
            unsafe {
                std::env::set_var(
                    "SEMA_OTEL_CAPTURE_CONTENT",
                    if c { "true" } else { "false" },
                )
            };
        }
    }
}

/// The guard for a `configure`-installed provider, held for the process so its `Drop`
/// (which flushes + shuts down) does NOT fire mid-run. Flush-at-exit is still covered by
/// the `atexit` hook registered in `install_provider`.
static CONFIGURED_GUARD: std::sync::Mutex<Option<OtelGuard>> = std::sync::Mutex::new(None);

/// Configure + install telemetry from Sema code (`otel/configure`). Applies `cfg` to the
/// environment, then runs the ordinary install path. Returns `true` when a provider was
/// installed by this call, `false` when nothing was configured or one was already
/// installed (by env at startup or an earlier `configure`) — programmatic config can
/// only win if the environment didn't already set telemetry up. Never panics.
pub fn configure(cfg: &OtelConfig) -> bool {
    cfg.apply_to_env();
    match install_provider() {
        Some(guard) => {
            // Retain the guard for the process; dropping it now would flush + tear the
            // provider down immediately. `atexit` still flushes on exit.
            if let Ok(mut slot) = CONFIGURED_GUARD.lock() {
                *slot = Some(guard);
            } else {
                std::mem::forget(guard);
            }
            true
        }
        None => false,
    }
}

/// Embedded mode: emit against whatever provider the host already installed in
/// `opentelemetry::global` (Decision #12). Installs nothing; silent no-op if the host
/// installed nothing. Enables the facade so it resolves the global tracer lazily.
pub fn use_host_global() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Process-lived multi-thread tokio runtime, created lazily ONLY when the gRPC
/// (tonic) OTLP path is used. tonic's channel + the async-runtime span/metric
/// processors need a live reactor on the export thread; HTTP (reqwest-blocking) and
/// the JSONL sink need none, so they never touch this. Never dropped (process-static)
/// so flush/shutdown at exit can't deadlock. `None` if the runtime fails to build
/// (degrade silently — never panic on the OTel path).
static OTEL_RT: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
fn otel_runtime() -> Option<&'static tokio::runtime::Runtime> {
    OTEL_RT
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("sema-otel")
                .enable_all()
                .build()
                .ok()
        })
        .as_ref()
}

fn otlp_protocol() -> String {
    std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_else(|_| "http/protobuf".into())
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

    let mut builder = SdkTracerProvider::builder().with_resource(build_resource());

    if let Some(path) = file {
        if let Ok(exp) = JsonlFileExporter::new(&path) {
            // Simple exporter → deterministic immediate JSONL capture.
            builder = builder.with_simple_exporter(exp);
        }
    }
    if otlp.is_some() {
        builder = attach_otlp_span_exporter(builder);
    }
    Some(builder.build())
}

/// Attach the OTLP span exporter per `OTEL_EXPORTER_OTLP_PROTOCOL` (Decision #3).
/// HTTP/JSON stay on the lightweight thread-based `BatchSpanProcessor` (no reactor).
/// gRPC uses the async-runtime processor on the static tokio runtime so tonic exports
/// actually run; the `rt.enter()` guard is held across BOTH builds (channel + worker).
fn attach_otlp_span_exporter(
    builder: opentelemetry_sdk::trace::TracerProviderBuilder,
) -> opentelemetry_sdk::trace::TracerProviderBuilder {
    use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig};
    match otlp_protocol().as_str() {
        "grpc" => {
            if let Some(rt) = otel_runtime() {
                let _g = rt.enter();
                if let Ok(exp) = SpanExporter::builder().with_tonic().build() {
                    let proc = opentelemetry_sdk::trace::span_processor_with_async_runtime::BatchSpanProcessor::builder(
                        exp,
                        opentelemetry_sdk::runtime::Tokio,
                    )
                    .build();
                    return builder.with_span_processor(proc);
                }
            }
            builder
        }
        "http/json" => match SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpJson)
            .build()
        {
            Ok(exp) => builder.with_batch_exporter(exp),
            Err(_) => builder,
        },
        _ => match SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .build()
        {
            Ok(exp) => builder.with_batch_exporter(exp),
            Err(_) => builder,
        },
    }
}

/// Build a meter provider (OTLP only — the JSONL sink is trace-only). `None` when no
/// OTLP endpoint is configured or the exporter fails to build.
fn build_meter_provider() -> Option<SdkMeterProvider> {
    let otlp = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT").ok())
        .filter(|s| !s.is_empty());
    otlp.as_ref()?;

    use opentelemetry_otlp::{MetricExporter, Protocol, WithExportConfig};
    match otlp_protocol().as_str() {
        "grpc" => {
            let rt = otel_runtime()?;
            let _g = rt.enter();
            let exp = MetricExporter::builder().with_tonic().build().ok()?;
            // Async-runtime periodic reader on the static tokio runtime so tonic exports run.
            let reader = opentelemetry_sdk::metrics::periodic_reader_with_async_runtime::PeriodicReader::builder(
                exp,
                opentelemetry_sdk::runtime::Tokio,
            )
            .build();
            Some(
                SdkMeterProvider::builder()
                    .with_reader(reader)
                    .with_resource(build_resource())
                    .build(),
            )
        }
        proto => {
            let exp = MetricExporter::builder()
                .with_http()
                .with_protocol(if proto == "http/json" {
                    Protocol::HttpJson
                } else {
                    Protocol::HttpBinary
                })
                .build()
                .ok()?;
            Some(
                SdkMeterProvider::builder()
                    .with_reader(PeriodicReader::builder(exp).build())
                    .with_resource(build_resource())
                    .build(),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Span core
// ---------------------------------------------------------------------------

/// Shared RAII span core. Holds the `Context` carrying the span (cloned onto the TL
/// stack for nesting). `Drop` pops the stack and ends the span (recording duration).
///
/// A `detached` core was NOT pushed onto the stack (it is a leaf whose parent was
/// captured at construction), so its `Drop` must NOT pop — popping would remove an
/// unrelated span, corrupting the stack. Used by the async embeddings span, whose
/// `Drop` runs on the VM thread inside the IO poller, possibly while a sibling
/// task's span is on the stack.
struct SpanCore {
    ctx: Context,
    detached: bool,
}

impl Drop for SpanCore {
    fn drop(&mut self) {
        if !self.detached {
            STACK.with(|s| {
                s.borrow_mut().pop();
            });
        }
        self.ctx.span().end();
    }
}

impl SpanCore {
    /// The OTel `Context` carrying this span (for temporarily re-installing a
    /// detached span as the active parent — see `LlmSpan::entered`).
    fn context(&self) -> &Context {
        &self.ctx
    }
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
    /// Apply a batch of attributes (used by the compat layer).
    fn set_attrs(&self, kvs: Vec<KeyValue>) {
        if kvs.is_empty() {
            return;
        }
        let span = self.ctx.span();
        for kv in kvs {
            span.set_attribute(kv);
        }
    }
    fn record_error(&self, kind: &str, msg: &str) {
        self.ctx
            .span()
            .set_attribute(KeyValue::new("error.type", kind.to_string()));
        self.ctx.span().set_status(Status::error(msg.to_string()));
    }
}

/// Build a span on `tracer` parented to `parent`, returning the `Context` that carries
/// it. Generic so it works for both the global `BoxedTracer` and an owned `SdkTracer`.
fn build_ctx<T: Tracer>(
    tracer: &T,
    name: String,
    kind: SpanKind,
    attrs: Vec<KeyValue>,
    parent: &Context,
) -> Context
where
    T::Span: Send + Sync + 'static,
{
    let span = tracer
        .span_builder(name)
        .with_kind(kind)
        .with_attributes(attrs)
        .start_with_context(tracer, parent);
    parent.with_span(span)
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
    // Route through a host-supplied provider (OwnProvider mode) when set; else the
    // global provider (standalone install or host's ambient provider).
    let owned = OWNED_PROVIDER.with(|c| c.borrow().clone());
    let ctx = match owned {
        Some(p) => build_ctx(&p.tracer_with_scope(scope()), name, kind, attrs, &parent),
        None => build_ctx(
            &global::tracer_with_scope(scope()),
            name,
            kind,
            attrs,
            &parent,
        ),
    };
    STACK.with(|s| s.borrow_mut().push(ctx.clone()));
    let core = SpanCore {
        ctx,
        detached: false,
    };
    // Apply the active conversation/session/user identity to EVERY span uniformly.
    if let Some(id) = CONVERSATION_ID.with(|c| c.borrow().clone()) {
        core.set_str("gen_ai.conversation.id", id);
    }
    if let Some(id) = SESSION_ID.with(|c| c.borrow().clone()) {
        core.set_str("session.id", id); // Langfuse session grouping
    }
    if let Some(id) = USER_ID.with(|c| c.borrow().clone()) {
        core.set_str("user.id", id); // Langfuse user attribution
    }
    // Per-backend trace identity (LangSmith session id, Langfuse release). No-op unless
    // the relevant compat backend is active.
    let session = SESSION_ID.with(|c| c.borrow().clone());
    core.set_attrs(compat::identity(session.as_deref(), release()));
    Some(core)
}

/// Start a DETACHED span: parented to the current TL-stack top (captured now) but
/// NOT pushed onto the stack, and whose `Drop` does NOT pop. For a leaf span whose
/// lifetime is decoupled from the stack discipline — e.g. an async embeddings span
/// carried into an IO-poller closure and finalized on the VM thread after a yield,
/// possibly while a sibling task's span sits on the stack. Returns `None` when
/// telemetry is disabled.
fn start_detached(name: String, kind: SpanKind, attrs: Vec<KeyValue>) -> Option<SpanCore> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let parent = STACK.with(|s| s.borrow().last().cloned());
    let parent = parent.unwrap_or_else(Context::current);
    let owned = OWNED_PROVIDER.with(|c| c.borrow().clone());
    let ctx = match owned {
        Some(p) => build_ctx(&p.tracer_with_scope(scope()), name, kind, attrs, &parent),
        None => build_ctx(
            &global::tracer_with_scope(scope()),
            name,
            kind,
            attrs,
            &parent,
        ),
    };
    // NOTE: no STACK push — this is the whole point of "detached".
    let core = SpanCore {
        ctx,
        detached: true,
    };
    if let Some(id) = CONVERSATION_ID.with(|c| c.borrow().clone()) {
        core.set_str("gen_ai.conversation.id", id);
    }
    if let Some(id) = SESSION_ID.with(|c| c.borrow().clone()) {
        core.set_str("session.id", id);
    }
    if let Some(id) = USER_ID.with(|c| c.borrow().clone()) {
        core.set_str("user.id", id);
    }
    let session = SESSION_ID.with(|c| c.borrow().clone());
    core.set_attrs(compat::identity(session.as_deref(), release()));
    Some(core)
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

/// Public: whether content capture is on, so callers can skip serializing content they
/// would only attach when capture is enabled (tool args/result, trace I/O rollup).
pub fn content_capture_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) && capture_content()
}

/// Backstop cap for already-per-field-truncated structured content before it becomes a
/// span attribute. Cheap and panic-proof (runs on the calling thread).
fn scrub(s: &str) -> String {
    const MAX: usize = 65_536;
    if s.len() <= MAX {
        return s.to_string();
    }
    // Byte-bounded, backed off to a char boundary for valid UTF-8.
    let mut end = MAX;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated]", &s[..end])
}

/// Format a `SystemTime` as RFC3339 UTC (`YYYY-MM-DDTHH:MM:SS.mmmZ`), dependency-free.
/// Used for the streaming first-token timestamp (Langfuse `completion_start_time`).
fn rfc3339(t: std::time::SystemTime) -> String {
    let dur = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let millis = dur.subsec_millis();
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (tod / 3_600, (tod % 3_600) / 60, tod % 60);
    // Days since 1970-01-01 → civil (Y/M/D) via Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{millis:03}Z")
}

/// Optional release/version stamp (Langfuse `langfuse.release`), read once from the env.
fn release() -> Option<&'static str> {
    static REL: OnceLock<Option<String>> = OnceLock::new();
    REL.get_or_init(|| std::env::var("SEMA_OTEL_RELEASE").ok())
        .as_deref()
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
    /// User-supplied tags, merged with auto-derived ones and emitted in `set_response`.
    user_tags: RefCell<Vec<String>>,
    /// Guards `mark_first_token` so streaming TTFT is recorded exactly once.
    first_token: std::cell::Cell<bool>,
}

/// Start an LLM-call span (CLIENT). Provider/model are unknown at entry and set later
/// via `set_dispatch`.
pub fn llm_span(op: &'static str) -> LlmSpan {
    let inner = start(
        op.to_string(),
        SpanKind::Client,
        vec![KeyValue::new("gen_ai.operation.name", op)],
    );
    if let Some(c) = &inner {
        let kind = if op == "embeddings" {
            compat::Kind::Embedding
        } else {
            compat::Kind::Llm
        };
        c.set_attrs(compat::span_kind(kind));
    }
    LlmSpan {
        inner,
        op,
        start: std::time::Instant::now(),
        dims: RefCell::new((String::new(), String::new())),
        user_tags: RefCell::new(Vec::new()),
        first_token: std::cell::Cell::new(false),
    }
}

/// Start a DETACHED LLM-call span (CLIENT) — parented to the current span but NOT
/// pushed onto the active-span stack (and its `Drop` does not pop). For the async
/// embeddings leaf, whose `LlmSpan` is moved into the IO-poller closure and
/// finalized on the VM thread after the task yields (when the stack may hold a
/// sibling task's span). The embeddings span is a leaf — it needs no nesting, only
/// a correctly-captured parent.
pub fn llm_span_detached(op: &'static str) -> LlmSpan {
    let inner = start_detached(
        op.to_string(),
        SpanKind::Client,
        vec![KeyValue::new("gen_ai.operation.name", op)],
    );
    if let Some(c) = &inner {
        let kind = if op == "embeddings" {
            compat::Kind::Embedding
        } else {
            compat::Kind::Llm
        };
        c.set_attrs(compat::span_kind(kind));
    }
    LlmSpan {
        inner,
        op,
        start: std::time::Instant::now(),
        dims: RefCell::new((String::new(), String::new())),
        user_tags: RefCell::new(Vec::new()),
        first_token: std::cell::Cell::new(false),
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
            c.set_attrs(compat::request_params(
                temperature,
                max_tokens,
                stop_sequences,
                reasoning_effort,
            ));
        }
    }

    /// Set `gen_ai.output.type` (`json` when JSON mode is requested, else `text`).
    pub fn set_output_type(&self, json: bool) {
        if let Some(c) = &self.inner {
            c.set_str("gen_ai.output.type", if json { "json" } else { "text" });
        }
    }

    /// Advertise the tools available this turn (compat: OpenInference `llm.tools.*`,
    /// Traceloop `llm.request.functions.*`). No-op unless compat is active.
    pub fn set_tools(&self, tools: &[crate::ToolView]) {
        if let Some(c) = &self.inner {
            c.set_attrs(compat::tools(tools));
        }
    }

    /// Advertise the input texts of an embeddings call (compat: OpenInference
    /// `embedding.embeddings.{i}.embedding.text`). Content-gated; raw vectors are never
    /// emitted.
    pub fn set_embedding_input(&self, texts: &[String]) {
        if let Some(c) = &self.inner {
            c.set_attrs(compat::embedding_texts(texts, capture_content()));
        }
    }

    /// Trace-level I/O rollup on a STANDALONE chat span (compat: Langfuse trace panel).
    /// Content-gated.
    pub fn set_trace_io(&self, input: &str, output: &str) {
        if !capture_content() {
            return;
        }
        if let Some(c) = &self.inner {
            c.set_attrs(compat::trace_io(input, output));
        }
    }

    /// Stash user tags. Auto-derived tags (provider/model/operation/cache-hit) are
    /// merged with these and emitted together in `set_response` (compat:
    /// Langfuse/LangSmith/Braintrust).
    pub fn set_tags(&self, tags: &[String]) {
        self.user_tags.borrow_mut().extend_from_slice(tags);
    }

    /// User metadata, fanned out per backend prefix (compat: Langfuse/LangSmith/
    /// Traceloop/Braintrust). Emitted immediately — it has no auto-derived component.
    pub fn set_metadata(&self, meta: &[(String, String)]) {
        if let Some(c) = &self.inner {
            c.set_attrs(compat::metadata(meta));
        }
    }

    /// Record streaming time-to-first-token (first call wins; later calls no-op). Emits
    /// the always-on vendor-neutral `sema.gen_ai.*` markers plus the backend-native keys
    /// (Langfuse `completion_start_time`, OpenLLMetry `gen_ai.is_streaming`).
    pub fn mark_first_token(&self) {
        if self.first_token.replace(true) {
            return;
        }
        let ttft = self.start.elapsed().as_secs_f64();
        if let Some(c) = &self.inner {
            c.set_bool("sema.gen_ai.is_streaming", true);
            c.set_f64("sema.gen_ai.server.time_to_first_token", ttft);
            c.set_attrs(compat::streaming(&rfc3339(std::time::SystemTime::now())));
        }
    }

    /// Merge auto-derived tags (provider/model/operation/cache-hit) with any user tags
    /// and emit them. Auto-tags need provider+model (from `set_dispatch`) and cache-hit
    /// (known at response), so this runs from `set_response`.
    fn emit_tags(&self, cache_hit: bool) {
        let Some(c) = &self.inner else { return };
        let (provider, model) = self.dims.borrow().clone();
        let mut tags = vec![format!("operation:{}", self.op)];
        if !provider.is_empty() {
            tags.push(format!("provider:{provider}"));
        }
        if !model.is_empty() {
            tags.push(format!("model:{model}"));
        }
        if cache_hit {
            tags.push("cache-hit".to_string());
        }
        tags.extend(self.user_tags.borrow().iter().cloned());
        c.set_attrs(compat::tags(&tags));
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
            // Compat aliases use the RAW Sema provider name (back-translated per backend).
            c.set_attrs(compat::llm_dispatch(self.op, sema_provider, request_model));
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
            c.set_i64(
                "gen_ai.usage.total_tokens",
                (facts.input_tokens + facts.output_tokens) as i64,
            );
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
                // De-facto community attribute (Decision #2) AND the key Langfuse maps
                // (`gen_ai.usage.cost`) — emit both so cost renders everywhere.
                c.set_f64("gen_ai.usage.cost_usd", cost);
                c.set_f64("gen_ai.usage.cost", cost);
            }
            if facts.cache_hit {
                // Vendor-prefixed: not a registered gen_ai.* attribute (mirrors
                // sema.gen_ai.request.reasoning_effort).
                c.set_bool("sema.gen_ai.cache.hit", true);
            }
            c.set_attrs(compat::llm_usage(
                facts.input_tokens,
                facts.output_tokens,
                facts.input_tokens + facts.output_tokens,
                facts.cache_read_input_tokens,
                facts.cache_creation_input_tokens,
                facts.cost_usd,
                facts.cost_prompt_usd,
                facts.cost_completion_usd,
            ));
            // Name the embedding model on the embeddings span (OpenInference).
            if self.op == "embeddings" {
                c.set_attrs(compat::embedding_model(&facts.response_model));
            }
        }
        // Auto-tags (provider/model/operation/cache-hit) + any user tags, emitted once
        // now that dispatch + response facts are both known.
        self.emit_tags(facts.cache_hit);
        // A cache hit made no provider call (zero usage, no serving provider) — recording
        // the histograms would emit a misleading zero-token sample with an empty provider
        // dimension. Skip metrics; the span (with sema.gen_ai.cache.hit) still records it.
        if !facts.cache_hit {
            self.record_metrics(facts);
        }
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
            let input = scrub(input);
            let output = scrub(output);
            // OTel GenAI structured form (Logfire/Braintrust/vanilla OTel read these).
            c.set_str("gen_ai.input.messages", input.clone());
            c.set_str("gen_ai.output.messages", output.clone());
            // Langfuse maps observation I/O from these keys (it ignores gen_ai.*.messages),
            // so the content actually renders on the generation.
            c.set_str("langfuse.observation.input", input.clone());
            c.set_str("langfuse.observation.output", output.clone());
            if let Some(sys) = system {
                c.set_str("gen_ai.system_instructions", scrub(sys));
            }
            // OpenInference input.value/output.value, Traceloop entity.input/output, etc.
            c.set_attrs(compat::io(&input, &output));
        }
    }

    pub fn record_error(&self, kind: &str, msg: &str) {
        if let Some(c) = &self.inner {
            c.record_error(kind, msg);
        }
    }

    /// Run `f` with this span pushed onto the active-span stack so any child spans
    /// (e.g. `retry_span`) parent under it, then pop. For finalizing a DETACHED
    /// span on the VM thread (it was never on the stack): a sibling task's span may
    /// be on the stack, so we must temporarily install ours as the parent context
    /// and restore on exit. A no-op when telemetry is disabled.
    pub fn entered<R>(&self, f: impl FnOnce() -> R) -> R {
        let Some(core) = &self.inner else {
            return f();
        };
        STACK.with(|s| s.borrow_mut().push(core.context().clone()));
        let r = f();
        STACK.with(|s| {
            s.borrow_mut().pop();
        });
        r
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
    if let Some(c) = &inner {
        c.set_attrs(compat::span_kind(compat::Kind::Tool));
    }
    ToolSpan { inner }
}

impl ToolSpan {
    pub fn set_conversation_id(&self, id: &str) {
        if let Some(c) = &self.inner {
            c.set_str("gen_ai.conversation.id", id.to_string());
        }
    }
    /// Tool call arguments + result. Canonical `gen_ai.tool.call.arguments`/`result`
    /// plus compat aliases. Content-gated.
    pub fn set_tool_io(&self, args_json: &str, result: &str) {
        if !capture_content() {
            return;
        }
        if let Some(c) = &self.inner {
            c.set_str("gen_ai.tool.call.arguments", args_json.to_string());
            c.set_str("gen_ai.tool.call.result", result.to_string());
            c.set_attrs(compat::tool_io(args_json, result));
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
    if let Some(c) = &inner {
        c.set_attrs(compat::span_kind(compat::Kind::Agent));
    }
    AgentSpan { inner }
}

impl AgentSpan {
    pub fn set_conversation_id(&self, id: &str) {
        if let Some(c) = &self.inner {
            c.set_str("gen_ai.conversation.id", id.to_string());
        }
    }
    /// Trace-level I/O rollup on the agent root (compat: Langfuse trace panel).
    /// Content-gated.
    pub fn set_trace_io(&self, input: &str, output: &str) {
        if !capture_content() {
            return;
        }
        if let Some(c) = &self.inner {
            c.set_attrs(compat::trace_io(input, output));
        }
    }
    pub fn set_tags(&self, tags: &[String]) {
        if let Some(c) = &self.inner {
            c.set_attrs(compat::tags(tags));
        }
    }
    pub fn set_metadata(&self, meta: &[(String, String)]) {
        if let Some(c) = &self.inner {
            c.set_attrs(compat::metadata(meta));
        }
    }
    pub fn record_error(&self, kind: &str, msg: &str) {
        if let Some(c) = &self.inner {
            c.record_error(kind, msg);
        }
    }
}

// ---------------------------------------------------------------------------
// Retriever + reranker spans (RAG; OpenInference RETRIEVER / RERANKER)
// ---------------------------------------------------------------------------

pub struct RetrieverSpan {
    inner: Option<SpanCore>,
}

/// Start an INTERNAL retrieval span (vector-store search returning documents).
pub fn retriever_span(query_dims: usize, k: usize) -> RetrieverSpan {
    let inner = start(
        "retrieve".to_string(),
        SpanKind::Internal,
        vec![
            KeyValue::new("sema.retrieval.query_dims", query_dims as i64),
            KeyValue::new("sema.retrieval.top_k", k as i64),
        ],
    );
    if let Some(c) = &inner {
        c.set_attrs(compat::span_kind(compat::Kind::Retriever));
    }
    RetrieverSpan { inner }
}

impl RetrieverSpan {
    /// `docs` is `(id, content, score)` per result. Content is content-gated.
    pub fn set_documents(&self, docs: &[(String, String, f64)]) {
        if let Some(c) = &self.inner {
            c.set_attrs(compat::retrieval_documents(docs, capture_content()));
        }
    }
    pub fn record_error(&self, kind: &str, msg: &str) {
        if let Some(c) = &self.inner {
            c.record_error(kind, msg);
        }
    }
}

pub struct RerankerSpan {
    inner: Option<SpanCore>,
}

/// Start an INTERNAL reranker span (cross-encoder reordering of candidate documents).
pub fn reranker_span(query: &str, model: &str, top_k: Option<usize>) -> RerankerSpan {
    let inner = start("rerank".to_string(), SpanKind::Internal, Vec::new());
    if let Some(c) = &inner {
        c.set_attrs(compat::span_kind(compat::Kind::Reranker));
        c.set_attrs(compat::reranker_meta(
            query,
            model,
            top_k,
            capture_content(),
        ));
    }
    RerankerSpan { inner }
}

impl RerankerSpan {
    /// Candidate documents fed to the reranker (content-gated).
    pub fn set_input(&self, docs: &[String]) {
        if let Some(c) = &self.inner {
            let docs: Vec<(String, Option<f64>)> = docs.iter().map(|d| (d.clone(), None)).collect();
            c.set_attrs(compat::reranker_documents(
                "input_documents",
                &docs,
                capture_content(),
            ));
        }
    }
    /// Reordered output documents with relevance scores (content-gated; scores always).
    pub fn set_output(&self, docs: &[(String, f64)]) {
        if let Some(c) = &self.inner {
            let docs: Vec<(String, Option<f64>)> =
                docs.iter().map(|(d, s)| (d.clone(), Some(*s))).collect();
            c.set_attrs(compat::reranker_documents(
                "output_documents",
                &docs,
                capture_content(),
            ));
        }
    }
    pub fn record_error(&self, kind: &str, msg: &str) {
        if let Some(c) = &self.inner {
            c.record_error(kind, msg);
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
    if let Some(c) = &inner {
        c.set_attrs(compat::span_kind(compat::Kind::Chain));
    }
    VmSpan { inner }
}

impl VmSpan {
    pub fn set_str(&self, key: &'static str, val: &str) {
        if let Some(c) = &self.inner {
            c.set_str(key, val.to_string());
        }
    }
}

/// Emit one completed `gc.collect` span for a cycle-collector pass.
///
/// The collector's observer fires *after* the pass finished (the collector
/// cannot hold a span open — sema-core does not depend on sema-otel), so the
/// span is recorded retroactively: built with an explicit start time
/// (`now − duration`) and ended at `now`, giving it the pass's real extent on
/// the timeline. Parented to the TL-stack top, so GC passes nest under
/// whatever span was active when the safe point hit (agent turn, notebook
/// cell, tool call) and surface as root spans otherwise (e.g. interpreter
/// teardown). No-op when telemetry is disabled.
pub fn gc_pass_span(event: &sema_core::GcPassEvent) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let end = std::time::SystemTime::now();
    let start_time = end
        .checked_sub(std::time::Duration::from_nanos(event.duration_ns))
        .unwrap_or(end);
    let attrs = vec![
        KeyValue::new("gc.trigger", event.trigger.as_str()),
        KeyValue::new("gc.candidates", event.stats.candidates as i64),
        KeyValue::new("gc.traced", event.stats.traced as i64),
        KeyValue::new("gc.collected", event.stats.collected as i64),
        KeyValue::new("gc.pruned", event.stats.pruned as i64),
        KeyValue::new("gc.registry_before", event.registry_len_before as i64),
        KeyValue::new("gc.aborted", event.stats.aborted),
    ];
    fn emit<T: Tracer>(
        tracer: &T,
        start_time: std::time::SystemTime,
        end: std::time::SystemTime,
        attrs: Vec<KeyValue>,
        parent: &Context,
    ) where
        T::Span: Send + Sync + 'static,
    {
        let mut span = tracer
            .span_builder("gc.collect")
            .with_kind(SpanKind::Internal)
            .with_start_time(start_time)
            .with_attributes(attrs)
            .start_with_context(tracer, parent);
        span.end_with_timestamp(end);
    }
    let parent = STACK.with(|s| s.borrow().last().cloned());
    let parent = parent.unwrap_or_else(Context::current);
    let owned = OWNED_PROVIDER.with(|c| c.borrow().clone());
    match owned {
        Some(p) => emit(
            &p.tracer_with_scope(scope()),
            start_time,
            end,
            attrs,
            &parent,
        ),
        None => emit(
            &global::tracer_with_scope(scope()),
            start_time,
            end,
            attrs,
            &parent,
        ),
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

// ---------------------------------------------------------------------------
// Sema-native tracing surface (the `otel/*` builtins): annotate the current span,
// and open user-typed spans that render like the built-in ones.
// ---------------------------------------------------------------------------

/// Build a `KeyValue` from a dynamic (user-supplied) key + scalar value.
fn attr_kv(key: String, val: crate::AttrValue) -> KeyValue {
    match val {
        crate::AttrValue::Str(s) => KeyValue::new(key, s),
        crate::AttrValue::Int(i) => KeyValue::new(key, i),
        crate::AttrValue::Float(f) => KeyValue::new(key, f),
        crate::AttrValue::Bool(b) => KeyValue::new(key, b),
    }
}

/// Set one attribute on the innermost active span (TL-stack top). No-op when disabled,
/// when there is no active span, or for a prototype-polluting key.
pub fn set_current_attr(key: &str, val: crate::AttrValue) {
    if !ENABLED.load(Ordering::Relaxed) || is_polluting_key(key) {
        return;
    }
    STACK.with(|s| {
        if let Some(ctx) = s.borrow().last() {
            ctx.span().set_attribute(attr_kv(key.to_string(), val));
        }
    });
}

/// Set many attributes on the innermost active span.
pub fn set_current_attrs(attrs: Vec<(String, crate::AttrValue)>) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    STACK.with(|s| {
        if let Some(ctx) = s.borrow().last() {
            let span = ctx.span();
            for (k, v) in attrs {
                if !is_polluting_key(&k) {
                    span.set_attribute(attr_kv(k, v));
                }
            }
        }
    });
}

/// Set the status of the innermost active span: `Some(msg)` = error (also records
/// `error.type`), `None` = ok.
pub fn set_current_status(error: Option<&str>) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    STACK.with(|s| {
        if let Some(ctx) = s.borrow().last() {
            let span = ctx.span();
            match error {
                Some(msg) => {
                    span.set_attribute(KeyValue::new("error.type", "error"));
                    span.set_status(Status::error(msg.to_string()));
                }
                None => span.set_status(Status::Ok),
            }
        }
    });
}

/// Record LLM token usage + cost on the innermost active span (for a user-built
/// `otel/llm-span`). Emits the same `gen_ai.usage.*` keys as the built-in LLM span plus
/// the active backend's compat aliases, so a custom call accounts identically.
pub fn set_current_llm_usage(input: u32, output: u32, cost: Option<f64>) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let total = input + output;
    STACK.with(|s| {
        if let Some(ctx) = s.borrow().last() {
            let span = ctx.span();
            span.set_attribute(KeyValue::new("gen_ai.usage.input_tokens", input as i64));
            span.set_attribute(KeyValue::new("gen_ai.usage.output_tokens", output as i64));
            span.set_attribute(KeyValue::new("gen_ai.usage.total_tokens", total as i64));
            if let Some(c) = cost {
                span.set_attribute(KeyValue::new("gen_ai.usage.cost_usd", c));
                span.set_attribute(KeyValue::new("gen_ai.usage.cost", c));
            }
            for kv in compat::llm_usage(input, output, total, 0, 0, cost, None, None) {
                span.set_attribute(kv);
            }
        }
    });
}

/// Map a user span flavor to (OTel `SpanKind`, compat `Kind`, optional operation name).
fn user_kind_map(kind: crate::SemaSpanKind) -> (SpanKind, compat::Kind, Option<&'static str>) {
    match kind {
        crate::SemaSpanKind::Internal => (SpanKind::Internal, compat::Kind::Chain, None),
        crate::SemaSpanKind::Llm => (SpanKind::Client, compat::Kind::Llm, Some("chat")),
        crate::SemaSpanKind::Tool => (SpanKind::Internal, compat::Kind::Tool, Some("execute_tool")),
        crate::SemaSpanKind::Retrieval => (SpanKind::Internal, compat::Kind::Retriever, None),
        crate::SemaSpanKind::Embedding => (
            SpanKind::Internal,
            compat::Kind::Embedding,
            Some("embeddings"),
        ),
    }
}

/// Open a user-emitted span of the given flavor with extra attributes. Returns a `VmSpan`
/// (RAII; ends on drop). Typed flavors also set `gen_ai.operation.name` + the compat
/// span-kind so they classify like the built-ins in every backend.
pub fn user_span(
    name: &str,
    kind: crate::SemaSpanKind,
    attrs: Vec<(String, crate::AttrValue)>,
) -> VmSpan {
    let (span_kind, compat_kind, op) = user_kind_map(kind);
    let mut kvs = Vec::new();
    if let Some(op) = op {
        kvs.push(KeyValue::new("gen_ai.operation.name", op));
    }
    if matches!(kind, crate::SemaSpanKind::Tool) {
        kvs.push(KeyValue::new("gen_ai.tool.name", name.to_string()));
    }
    for (k, v) in attrs {
        if !is_polluting_key(&k) {
            kvs.push(attr_kv(k, v));
        }
    }
    let inner = start(name.to_string(), span_kind, kvs);
    if let Some(c) = &inner {
        c.set_attrs(compat::span_kind(compat_kind));
    }
    VmSpan { inner }
}

/// Open a user-built LLM/generation span (CLIENT). Sets `gen_ai.*` request attributes +
/// the compat dispatch aliases from the in-scope provider/model, so a custom provider call
/// renders as a first-class generation. Usage is added later via `set_current_llm_usage`.
pub fn user_llm_span(
    model: &str,
    provider: &str,
    operation: &str,
    attrs: Vec<(String, crate::AttrValue)>,
) -> VmSpan {
    let op = if operation.is_empty() {
        "chat"
    } else {
        operation
    };
    let mut kvs = vec![KeyValue::new("gen_ai.operation.name", op.to_string())];
    if !provider.is_empty() {
        kvs.push(KeyValue::new(
            "gen_ai.provider.name",
            crate::gen_ai_provider_name(provider).to_string(),
        ));
    }
    if !model.is_empty() {
        kvs.push(KeyValue::new("gen_ai.request.model", model.to_string()));
    }
    for (k, v) in attrs {
        if !is_polluting_key(&k) {
            kvs.push(attr_kv(k, v));
        }
    }
    let inner = start(op.to_string(), SpanKind::Client, kvs);
    if let Some(c) = &inner {
        c.set_attrs(compat::span_kind(compat::Kind::Llm));
        c.set_attrs(compat::llm_dispatch(op, provider, model));
    }
    VmSpan { inner }
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
        let big = "x".repeat(100_000);
        let out = scrub(&big);
        assert!(out.len() < big.len());
        assert!(out.ends_with("…[truncated]"));
    }

    #[test]
    fn rfc3339_matches_known_epochs() {
        let at = |secs: u64| rfc3339(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(at(1_735_689_600), "2025-01-01T00:00:00.000Z");
        // Leap day + end-of-day boundary.
        assert_eq!(at(1_709_164_800), "2024-02-29T00:00:00.000Z");
        assert_eq!(at(1_709_251_199), "2024-02-29T23:59:59.000Z");
        // Sub-second millis are preserved.
        assert_eq!(
            rfc3339(std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_500)),
            "1970-01-01T00:00:01.500Z"
        );
    }
}
