use super::*;

// ── I/O-overlap instrumentation ─────────────────────────────────
//
// Used only by the `llm/io-sleep-once` spike leaf (and its acceptance test) to
// prove that N offloaded futures are in flight *simultaneously* on SHARED_RT,
// not merely that the wall-clock was fast. `IO_INFLIGHT` is the live count;
// `IO_PEAK` is the high-water mark.
// Signed (`AtomicI64`), not `AtomicUsize`: an abandoned future (a task dropped by
// `async/timeout` or a pool error-path) still runs to completion and decrements the
// counter during a *later* test. On `usize` that underflows to `usize::MAX`, which
// then (a) panics on the regular `+ 1` below and (b) poisons `IO_PEAK`. Signed lets a
// stray decrement go to -1 harmlessly, and we clamp the decrement at 0 so it never
// shifts a later test's high-water mark.
#[cfg(not(target_arch = "wasm32"))]
pub static IO_INFLIGHT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
#[cfg(not(target_arch = "wasm32"))]
pub static IO_PEAK: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Peak number of `llm/io-sleep-once` futures simultaneously in flight. The
/// acceptance test asserts this is `>= 2` to prove true overlap.
#[cfg(not(target_arch = "wasm32"))]
pub fn io_peak_inflight() -> usize {
    IO_PEAK.load(std::sync::atomic::Ordering::SeqCst).max(0) as usize
}

/// Reset the spike in-flight counters (test helper).
#[cfg(not(target_arch = "wasm32"))]
pub fn reset_io_inflight() {
    IO_INFLIGHT.store(0, std::sync::atomic::Ordering::SeqCst);
    IO_PEAK.store(0, std::sync::atomic::Ordering::SeqCst);
}

/// How many raw cache/cassette filesystem sites executed while a runtime quantum was
/// active on the calling thread. Under the cooperative runtime this MUST stay 0 — the
/// disk legs are offloaded onto the blocking tier (cache read/write, cassette
/// load/save). The `debug_assert` in [`note_off_quantum_fs`] catches a regression in
/// debug builds; this counter is the release-safe seam the tests read.
pub(super) static QUANTUM_FS_CALLS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Guard for every raw cache/cassette filesystem site: they MUST run OFF the
/// cooperative runtime quantum (on a blocking-tier worker or the host thread). Fires
/// a `debug_assert` and bumps [`QUANTUM_FS_CALLS`] if invoked while a quantum is
/// active on this thread.
pub(crate) fn note_off_quantum_fs(site: &str) {
    let on_quantum = sema_core::in_runtime_quantum();
    if on_quantum {
        QUANTUM_FS_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    debug_assert!(
        !on_quantum,
        "{site}: cache/cassette filesystem I/O ran on the runtime quantum"
    );
}

/// Test seam: number of cache/cassette fs sites that ran on a quantum thread (must be
/// 0). See [`note_off_quantum_fs`].
#[doc(hidden)]
pub fn quantum_fs_calls() -> u64 {
    QUANTUM_FS_CALLS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Reset the quantum-fs seam counter (test helper).
#[doc(hidden)]
pub fn reset_quantum_fs_calls() {
    QUANTUM_FS_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// RAII gauge for one offloaded completion future: bumps `IO_INFLIGHT` + `IO_PEAK`
/// on construction and decrements (clamped at 0) on drop, so an abort that drops
/// the future before or during its first poll cannot strand the gauge at +1.
/// External-wait paths use this to prove simultaneity
/// (`io_peak_inflight() >= 2`).
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct InflightGuard;

#[cfg(not(target_arch = "wasm32"))]
impl InflightGuard {
    pub(super) fn new() -> Self {
        use std::sync::atomic::Ordering;
        let prev = IO_INFLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        IO_PEAK.fetch_max(prev, Ordering::SeqCst);
        InflightGuard
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for InflightGuard {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        let _ =
            IO_INFLIGHT.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some((v - 1).max(0)));
    }
}

pub(super) fn set_serving_provider(name: &str) {
    LAST_SERVING_PROVIDER.with(|p| *p.borrow_mut() = Some(name.to_string()));
}

pub(super) fn take_serving_provider() -> Option<String> {
    LAST_SERVING_PROVIDER.with(|p| p.borrow_mut().take())
}

/// Snapshot this thread's cumulative session LLM cost in USD — the same figure
/// `(llm/session-usage)` reports as `:cost-usd`. Zeroed by
/// [`reset_runtime_state`], i.e. on interpreter construction.
pub fn session_cost_snapshot() -> f64 {
    SESSION_COST.with(|sc| *sc.borrow())
}

/// Reset LLM runtime state used by builtins.
/// Called by interpreter construction to avoid cross-instance leakage.
pub fn reset_runtime_state() {
    // Install THE process-wide I/O pool (ADR #69) so lib tests that reset LLM
    // state without a full interpreter still offload onto the one pool.
    sema_io::install();
    PROVIDER_REGISTRY.with(|r| *r.borrow_mut() = ProviderRegistry::new());
    SESSION_USAGE.with(|u| *u.borrow_mut() = Usage::default());
    LAST_USAGE.with(|u| *u.borrow_mut() = None);
    ACTIVE_LEAF_SCOPE.with(|s| *s.borrow_mut() = None);
    USAGE_ACCUM_SUPPRESS.with(|s| s.set(false));
    // Idempotently register the per-task usage-scope seam (fn-pointer thread-locals)
    // so the scheduler can swap the active leaf scope in/out per task step.
    register_usage_scope_task_callbacks();
    // Register the per-task LLM dynamic-scope seam so the scheduler can swap
    // cache, budget, cassette, and call metadata in/out per task step.
    register_llm_scope_task_callbacks();
    SESSION_COST.with(|c| *c.borrow_mut() = 0.0);
    ACTIVE_BUDGET.with(|b| *b.borrow_mut() = None);
    BUDGET_STACK.with(|s| s.borrow_mut().clear());
    STREAM_BUDGET_PREGATE.with(|c| c.set(false));
    PRICING_WARNING_SHOWN.with(|shown| shown.set(false));
    LISP_PROVIDERS.with(|p| p.borrow_mut().clear());
    CACHE_ENABLED.with(|c| c.set(false));
    CACHE_MEM.with(|c| c.borrow_mut().clear());
    CACHE_TTL_SECS.with(|c| c.set(3600));
    CACHE_HITS.with(|c| c.set(0));
    CACHE_MISSES.with(|c| c.set(0));
    FALLBACK_CHAIN.with(|c| *c.borrow_mut() = None);
    VECTOR_STORES.with(|s| s.borrow_mut().clear());
    RATE_LIMIT_RPS.with(|r| r.set(None));
    RATE_LIMIT_LAST.with(|r| *r.borrow_mut() = None);
    install_cassette_scope(None);
    ACTIVE_POLICIES.with(|policies| policies.borrow_mut().clear());
    POLICY_BYPASS.with(|bypass| bypass.borrow_mut().clear());
    POLICY_AGENT_ID.with(|agent| *agent.borrow_mut() = None);
    LAST_SERVING_PROVIDER.with(|p| *p.borrow_mut() = None);
    RETRY_BASE_MS.with(|c| c.set(500));
    NETWORK_MAX_RETRIES.with(|c| c.set(3));
    clear_agent_runs();
    clear_stream_runs();
    pricing::clear_custom_pricing();
}

// ── MCP-call cassette bridge ─────────────────────────────────
// The task-scoped LLM cassette also serves MCP `tools/call` interactions. The
// hook returns a recorder capability that retains the selected tape across an
// asynchronous call, without introducing a sema-mcp → sema-llm dependency.

/// Test-only: register `provider` as the default LLM provider, bypassing
/// `llm/configure`. Lets integration tests drive the completion/agent paths with
/// a scripted [`crate::fake::FakeProvider`] — no API keys, fully deterministic.
/// Call [`reset_runtime_state`] first to clear any prior provider.
pub fn register_test_provider(provider: Box<dyn LlmProvider>) {
    let name = provider.name().to_string();
    PROVIDER_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        reg.register(provider);
        reg.set_default(&name);
    });
}

pub(super) fn with_provider<F, R>(f: F) -> Result<R, SemaError>
where
    F: FnOnce(&dyn LlmProvider) -> Result<R, SemaError>,
{
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = reg.default_provider().ok_or_else(|| {
            SemaError::Llm(
                "no LLM provider configured. Use (llm/configure :anthropic {:api-key ...}) first"
                    .to_string(),
            )
        })?;
        f(&*provider)
    })
}

pub(super) fn with_embedding_provider<F, R>(f: F) -> Result<R, SemaError>
where
    F: FnOnce(&dyn LlmProvider) -> Result<R, SemaError>,
{
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = reg
            .embedding_provider()
            .or_else(|| reg.default_provider())
            .ok_or_else(|| {
                SemaError::Llm(
                    "no embedding provider configured. Use (llm/configure-embeddings ...) first"
                        .to_string(),
                )
            })?;
        f(&*provider)
    })
}

/// Pull a human-readable text snippet from a vector-store document's metadata
/// (`:text` or `:content`), for the retriever span's `document.content`. Empty if absent.
pub(super) fn metadata_text(metadata: &Value) -> String {
    let Some(m) = metadata.as_map_rc() else {
        return String::new();
    };
    for key in ["text", "content"] {
        if let Some(s) = m.get(&Value::keyword(key)).and_then(|v| v.as_str()) {
            return s.to_string();
        }
    }
    String::new()
}

pub(super) fn with_rerank_provider<F, R>(name: Option<&str>, f: F) -> Result<R, SemaError>
where
    F: FnOnce(&dyn LlmProvider) -> Result<R, SemaError>,
{
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = match name {
            Some(n) => reg
                .get(n)
                .ok_or_else(|| SemaError::Llm(format!("rerank provider '{n}' not found")))?,
            None => reg
                .rerank_provider()
                .or_else(|| reg.default_provider())
                .ok_or_else(|| {
                    SemaError::Llm(
                        "no rerank provider configured — set COHERE_API_KEY, JINA_API_KEY, or \
                         VOYAGE_API_KEY (or pass {:provider ...})"
                            .to_string(),
                    )
                })?,
        };
        f(&*provider)
    })
}
