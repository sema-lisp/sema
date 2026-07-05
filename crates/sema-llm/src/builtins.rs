use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use sema_core::{
    resolve, Agent, Conversation, Env, EvalContext, ImageAttachment, Message, NativeFn, Prompt,
    Role, SemaError, Value, ValueView,
};

use sha2::{Digest, Sha256};

use crate::anthropic::AnthropicProvider;
use crate::embeddings::{CohereEmbeddingProvider, OpenAiCompatEmbeddingProvider};
use crate::gemini::GeminiProvider;
use crate::ollama::OllamaProvider;
use crate::openai::OpenAiProvider;
use crate::pricing;
use crate::provider::{LlmProvider, ProviderRegistry};
use crate::types::{
    ChatMessage, ChatRequest, ChatResponse, ContentBlock, EmbedRequest, EmbedResponse, LlmError,
    RerankRequest, ToolCall, ToolSchema, Usage,
};
use crate::vector_store::{VectorDocument, VectorStore};

thread_local! {
    static PROVIDER_REGISTRY: RefCell<ProviderRegistry> = RefCell::new(ProviderRegistry::new());
    static SESSION_USAGE: RefCell<Usage> = RefCell::new(Usage::default());
    static LAST_USAGE: RefCell<Option<Usage>> = const { RefCell::new(None) };
    /// The per-leaf usage accumulator active for the CURRENT TASK. The workflow
    /// runtime opens a fresh frame (via [`open_usage_scope`]) around each agent leaf;
    /// `track_usage` folds the completion it records into this frame. Unlike the single
    /// `LAST_USAGE` slot, this survives concurrent parallel/pipeline fan-out: it is a
    /// per-TASK slot (captured at task spawn, swapped in/out at each task step via the
    /// `sema_core` usage-scope seam — mirroring the otel context), so a sibling leaf
    /// running concurrently can't clobber an in-flight leaf's tally. A multi-round tool
    /// loop SUMS every round's usage instead of seeing only the last. The async
    /// completion path additionally captures this frame's `Rc` into its poller closure
    /// so the fold lands even though the poller runs outside the task-step boundary.
    static ACTIVE_LEAF_SCOPE: RefCell<Option<Rc<RefCell<LeafUsage>>>> = const { RefCell::new(None) };
    /// Set while an async completion's poller folds usage into a CAPTURED frame Rc.
    /// Suppresses `track_usage`'s own active-frame fold so the async path counts each
    /// completion exactly once.
    static USAGE_ACCUM_SUPPRESS: Cell<bool> = const { Cell::new(false) };
    static SESSION_COST: RefCell<f64> = const { RefCell::new(0.0) };
    /// The budget frame in force for the CURRENT TASK, held behind a shared `Rc` so
    /// that all concurrent tasks spawned inside one `llm/with-budget` charge ONE
    /// aggregate frame (captured by-`Rc` onto each task at spawn via the per-task LLM
    /// dynamic-scope seam, and re-installed around the async completion poller's
    /// `track_usage`). `None` when no budget is active.
    static ACTIVE_BUDGET: RefCell<Option<Rc<RefCell<BudgetFrame>>>> = const { RefCell::new(None) };
    /// When set (via `llm/with-budget {:on-stream :pre-gate}`), `llm/stream` checks the
    /// budget BEFORE opening a stream (usage is unknown until a stream ends, so this is
    /// the only honest place to gate). Default off — streams don't enforce the budget.
    static STREAM_BUDGET_PREGATE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Saved outer budget frames for nested `llm/with-budget` scopes. A push installs a
    /// fresh frame; the pop restores the frame recorded here.
    static BUDGET_STACK: RefCell<Vec<Option<Rc<RefCell<BudgetFrame>>>>> = const { RefCell::new(Vec::new()) };
    /// Pluggable memory callbacks, set by `sema-stdlib` when it registers the memory
    /// module. Allows `agent/run` to seed from and append to a memory handle without
    /// depending on `sema-stdlib` (which would be circular).
    static MEMORY_CALLBACKS: RefCell<Option<MemoryCbs>> = const { RefCell::new(None) };
}

/// Function-pointer table injected by `sema-stdlib/memory.rs` via
/// [`register_memory_callbacks`]. Both slots are plain `fn` pointers (not closures)
/// so they satisfy the `'static` bound required by `thread_local!`.
struct MemoryCbs {
    get_working: fn(&Value) -> Result<Vec<crate::types::ChatMessage>, sema_core::SemaError>,
    append_back: fn(&Value, &[crate::types::ChatMessage]) -> Result<(), sema_core::SemaError>,
}

/// Register the memory integration callbacks. Called once by `sema-stdlib/memory.rs`
/// during its `register(env)` call. Uses plain `fn` pointers so the callbacks are
/// `'static` and thread-safe within the single-threaded runtime model.
pub fn register_memory_callbacks(
    get_working: fn(&Value) -> Result<Vec<crate::types::ChatMessage>, sema_core::SemaError>,
    append_back: fn(&Value, &[crate::types::ChatMessage]) -> Result<(), sema_core::SemaError>,
) {
    MEMORY_CALLBACKS.with(|c| {
        *c.borrow_mut() = Some(MemoryCbs {
            get_working,
            append_back,
        });
    });
}

/// A small snapshot of the most recent completion's usage, for callers (e.g. the
/// workflow runtime) that want to attribute tokens/cost to a step without depending
/// on the internal `Usage` type. `None` until a completion has run on this thread.
#[derive(Debug, Clone)]
pub struct LastUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model: String,
    /// `None` when pricing is unknown for the model (genuinely absent, not 0).
    pub cost_usd: Option<f64>,
}

/// Clear the per-thread last-usage slot. The workflow runtime calls this at the START
/// of each agent leaf so that [`last_usage_snapshot`] read afterwards reflects ONLY a
/// completion this leaf made — a leaf whose call fails (or makes none) reports `None`
/// instead of re-reading the previous leaf's usage (which would mis-attribute a budget
/// event and double-charge the cap).
pub fn clear_last_usage() {
    LAST_USAGE.with(|u| *u.borrow_mut() = None);
}

/// Snapshot the most recent LLM completion's usage on this thread (tokens + model
/// + computed cost). Used by the workflow runtime to emit per-agent budget events.
pub fn last_usage_snapshot() -> Option<LastUsage> {
    LAST_USAGE.with(|u| {
        u.borrow().as_ref().map(|usage| LastUsage {
            input_tokens: usage.prompt_tokens as u64,
            output_tokens: usage.completion_tokens as u64,
            model: usage.model.clone(),
            cost_usd: pricing::calculate_cost(usage),
        })
    })
}

/// One per-leaf usage tally, summed across every completion a single agent leaf makes
/// (e.g. every round of a multi-round tool loop). `calls == 0` means the leaf made no
/// (non-cache-hit) provider call — the workflow runtime then emits NO budget event,
/// honoring the cache-hit-zero-usage invariant (no phantom zero Budget event).
#[derive(Debug, Clone, Default)]
pub struct LeafUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Summed cost; `None` while no priced call has landed, `Some` once one has (a
    /// later unpriced call leaves the running sum unchanged).
    pub cost_usd: Option<f64>,
    /// The model of the most recent priced/recorded call in this scope.
    pub model: String,
    pub calls: u32,
}

/// Fold one completion's usage into a `LeafUsage` tally. A cache hit (all-zero usage)
/// does NOT increment `calls`, so a purely-cached leaf stays filtered downstream.
fn accumulate_into(slot: &Rc<RefCell<LeafUsage>>, usage: &Usage, cost: Option<f64>) {
    let input = usage.prompt_tokens as u64;
    let output = usage.completion_tokens as u64;
    // Cache-hit-zero-usage invariant: an all-zero, unpriced completion is a cache hit;
    // don't count it as a call (no phantom zero Budget event for a cached leaf).
    if input == 0 && output == 0 && cost.is_none() {
        return;
    }
    let mut acc = slot.borrow_mut();
    acc.input_tokens += input;
    acc.output_tokens += output;
    if let Some(c) = cost {
        acc.cost_usd = Some(acc.cost_usd.unwrap_or(0.0) + c);
    }
    if !usage.model.is_empty() {
        acc.model = usage.model.clone();
    }
    acc.calls += 1;
}

/// RAII handle for a per-leaf usage accumulator. Installs a fresh frame as the active
/// (per-task) scope on construction and restores the previously-active one on drop, so
/// sequential/nested leaves nest correctly. The workflow runtime reads
/// [`UsageScope::usage`] before drop to attribute tokens/cost to the leaf.
pub struct UsageScope {
    slot: Rc<RefCell<LeafUsage>>,
    /// The scope that was active before this one, restored on drop.
    prev: Option<Rc<RefCell<LeafUsage>>>,
}

impl UsageScope {
    /// Read the accumulated tally for this scope's leaf.
    pub fn usage(&self) -> LeafUsage {
        self.slot.borrow().clone()
    }
}

impl Drop for UsageScope {
    fn drop(&mut self) {
        ACTIVE_LEAF_SCOPE.with(|s| *s.borrow_mut() = self.prev.take());
    }
}

/// Open a per-leaf usage accumulation scope. `track_usage` folds each completion made
/// while the returned guard is alive into this scope's frame; the async completion path
/// captures the frame's `Rc` into its poller so an in-flight leaf is tallied even after
/// a sibling task runs. The guard restores the prior active scope on drop.
pub fn open_usage_scope() -> UsageScope {
    let slot = Rc::new(RefCell::new(LeafUsage::default()));
    let prev = ACTIVE_LEAF_SCOPE.with(|s| s.borrow_mut().replace(Rc::clone(&slot)));
    UsageScope { slot, prev }
}

/// Clone the active (per-task) usage-accumulator frame's `Rc`, if any. The async
/// completion path captures this at yield time so the poller folds usage into the
/// LEAF'S OWN frame — correct even across a concurrent sibling task.
fn current_usage_accum() -> Option<Rc<RefCell<LeafUsage>>> {
    ACTIVE_LEAF_SCOPE.with(|s| s.borrow().clone())
}

// ── Per-task active-leaf-scope seam (registered into sema-core) ─────
//
// The scheduler captures the active leaf scope at task spawn and swaps it in/out at
// each task step (just like the otel context), so an inline agent thunk inherits the
// scope its `workflow/step` opened and concurrent sibling tasks stay isolated. These
// fns are the type-erased bridge sema-core calls; the `Rc` is carried in a `Box<dyn
// Any>` holding `Option<Rc<RefCell<LeafUsage>>>`.

/// Capture (clone) the active leaf scope to seed onto a freshly-spawned task.
fn capture_usage_scope() -> Box<dyn std::any::Any> {
    Box::new(ACTIVE_LEAF_SCOPE.with(|s| s.borrow().clone()))
}

/// Take the active leaf scope out of the thread-local (leaving none).
fn take_usage_scope() -> Box<dyn std::any::Any> {
    Box::new(ACTIVE_LEAF_SCOPE.with(|s| s.borrow_mut().take()))
}

/// Install a leaf scope into the thread-local, returning the one displaced.
fn install_usage_scope(ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
    let incoming: Option<Rc<RefCell<LeafUsage>>> = ctx
        .downcast::<Option<Rc<RefCell<LeafUsage>>>>()
        .map(|b| *b)
        .unwrap_or(None);
    Box::new(ACTIVE_LEAF_SCOPE.with(|s| std::mem::replace(&mut *s.borrow_mut(), incoming)))
}

/// Register the per-task usage-scope callbacks with sema-core. Called once at startup.
pub fn register_usage_scope_task_callbacks() {
    sema_core::set_usage_scope_task_callbacks(
        capture_usage_scope,
        take_usage_scope,
        install_usage_scope,
    );
}

// ── Per-task LLM dynamic scope (cache / budget / tags) — ASYNC-1 ─────
//
// `llm/with-cache`, `llm/with-budget`, and per-call `:tags`/`:metadata` set
// dynamically-scoped thread-locals for the extent of a thunk, then reset them. A task
// spawned inside that thunk reads them WHEN IT RUNS — which the cooperative scheduler
// can defer past the reset. The scheduler captures this scope at `async/spawn` and
// swaps it in/out at each task step (like the otel context and leaf-usage scope), so
// concurrent siblings stay isolated. Read-only flags ride as a value snapshot; the
// budget frame rides as a shared `Rc` so all siblings in one `with-budget` charge one
// aggregate. Reached from `sema-core` through the type-erased fn-pointer seam.

/// The dynamically-scoped LLM state captured onto a task and swapped in/out per step.
struct LlmDynScope {
    cache_enabled: bool,
    cache_ttl_secs: i64,
    stream_budget_pregate: bool,
    call_tags: Vec<String>,
    call_meta: Vec<(String, String)>,
    /// The active budget frame, shared by `Rc` so concurrent siblings charge one aggregate.
    budget: Option<Rc<RefCell<BudgetFrame>>>,
}

impl Default for LlmDynScope {
    fn default() -> Self {
        LlmDynScope {
            cache_enabled: false,
            cache_ttl_secs: 3600,
            stream_budget_pregate: false,
            call_tags: Vec::new(),
            call_meta: Vec::new(),
            budget: None,
        }
    }
}

/// Read (clone) the current thread's LLM dynamic scope without disturbing it.
fn read_llm_scope() -> LlmDynScope {
    LlmDynScope {
        cache_enabled: CACHE_ENABLED.with(|c| c.get()),
        cache_ttl_secs: CACHE_TTL_SECS.with(|c| c.get()),
        stream_budget_pregate: STREAM_BUDGET_PREGATE.with(|c| c.get()),
        call_tags: CALL_TAGS.with(|t| t.borrow().clone()),
        call_meta: CALL_META.with(|m| m.borrow().clone()),
        budget: ACTIVE_BUDGET.with(|b| b.borrow().clone()),
    }
}

/// Overwrite the current thread's LLM dynamic scope with `s`, returning the previous one.
fn write_llm_scope(s: LlmDynScope) -> LlmDynScope {
    let prev = read_llm_scope();
    CACHE_ENABLED.with(|c| c.set(s.cache_enabled));
    CACHE_TTL_SECS.with(|c| c.set(s.cache_ttl_secs));
    STREAM_BUDGET_PREGATE.with(|c| c.set(s.stream_budget_pregate));
    CALL_TAGS.with(|t| *t.borrow_mut() = s.call_tags);
    CALL_META.with(|m| *m.borrow_mut() = s.call_meta);
    ACTIVE_BUDGET.with(|b| *b.borrow_mut() = s.budget);
    prev
}

/// Capture (clone) the LLM dynamic scope to seed onto a freshly-spawned task.
fn capture_llm_scope() -> Box<dyn std::any::Any> {
    Box::new(read_llm_scope())
}

/// Take the LLM dynamic scope out of the thread-locals, leaving defaults.
fn take_llm_scope() -> Box<dyn std::any::Any> {
    Box::new(write_llm_scope(LlmDynScope::default()))
}

/// Install an LLM dynamic scope into the thread-locals, returning the one displaced.
fn install_llm_scope(ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
    let incoming: LlmDynScope = ctx
        .downcast::<LlmDynScope>()
        .map(|b| *b)
        .unwrap_or_default();
    Box::new(write_llm_scope(incoming))
}

/// Register the per-task LLM dynamic-scope callbacks with sema-core. Called once at startup.
pub fn register_llm_scope_task_callbacks() {
    sema_core::set_llm_scope_task_callbacks(capture_llm_scope, take_llm_scope, install_llm_scope);
}

#[derive(Clone, Default)]
struct BudgetFrame {
    cost_limit: Option<f64>,
    cost_spent: f64,
    token_limit: Option<u64>,
    tokens_spent: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedResponse {
    content: String,
    model: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    cached_at: i64,
}

/// One entry in an `llm/with-fallback` chain: a provider name plus an optional
/// per-provider model override. When `model` is `Some`, that model id is used for
/// this provider regardless of any model pinned in the call body (chain override
/// wins) — this lets a chain target a different model per provider, e.g. Opus on
/// Anthropic but a GPT model on OpenAI. When `None`, the provider's configured
/// default model is used.
#[derive(Debug, Clone)]
struct FallbackEntry {
    provider: String,
    model: Option<String>,
}

thread_local! {
    static PRICING_WARNING_SHOWN: Cell<bool> = const { Cell::new(false) };
    static LISP_PROVIDERS: RefCell<std::collections::HashMap<String, LispProviderCallbacks>> = RefCell::new(std::collections::HashMap::new());
    static CACHE_ENABLED: Cell<bool> = const { Cell::new(false) };
    static CACHE_MEM: RefCell<std::collections::HashMap<String, CachedResponse>> =
        RefCell::new(std::collections::HashMap::new());
    static CACHE_TTL_SECS: Cell<i64> = const { Cell::new(3600) };
    static CACHE_HITS: Cell<u64> = const { Cell::new(0) };
    static CACHE_MISSES: Cell<u64> = const { Cell::new(0) };
    // Active LLM cassette (record/replay). Sits below the otel span + response
    // cache, above the real provider — see crate::cassette.
    static CASSETTE: RefCell<Option<crate::cassette::Cassette>> = const { RefCell::new(None) };
    static FALLBACK_CHAIN: RefCell<Option<Vec<FallbackEntry>>> = const { RefCell::new(None) };
    static VECTOR_STORES: RefCell<std::collections::HashMap<String, VectorStore>> =
        RefCell::new(std::collections::HashMap::new());
    static RATE_LIMIT_RPS: Cell<Option<f64>> = const { Cell::new(None) };
    static RATE_LIMIT_LAST: Cell<u64> = const { Cell::new(0) };
    // Name of the provider that served the most recent `do_complete` response, so cost
    // tracking can price the model as served by that provider (resellers/gateways can list
    // the same model id at a different rate). Set at the dispatch choke points, consumed +
    // cleared by `track_usage`. `None` → canonical first-party price.
    static LAST_SERVING_PROVIDER: RefCell<Option<String>> = const { RefCell::new(None) };
}

// ── AwaitIo spike instrumentation ───────────────────────────────
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

fn set_serving_provider(name: &str) {
    LAST_SERVING_PROVIDER.with(|p| *p.borrow_mut() = Some(name.to_string()));
}

fn take_serving_provider() -> Option<String> {
    LAST_SERVING_PROVIDER.with(|p| p.borrow_mut().take())
}

struct LispProviderCallbacks {
    complete_fn: Value,
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
    // Idempotently register the per-task LLM dynamic-scope seam (cache/budget/tags) so
    // the scheduler can swap it in/out per task step (ASYNC-1).
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
    RATE_LIMIT_LAST.with(|r| r.set(0));
    CASSETTE.with(|c| *c.borrow_mut() = None);
    LAST_SERVING_PROVIDER.with(|p| *p.borrow_mut() = None);
    RETRY_BASE_MS.with(|c| c.set(500));
    NETWORK_MAX_RETRIES.with(|c| c.set(3));
    clear_agent_runs();
    clear_stream_runs();
    pricing::clear_custom_pricing();
}

// ── MCP-call cassette bridge ─────────────────────────────────
// The LLM cassette (thread-local `CASSETTE`) also serves MCP `tools/call`
// interactions so agent-over-MCP flows replay deterministically. These fns are
// registered into `sema-core` (fn pointers) so `sema-mcp` can consult the tape
// without depending on `sema-llm`.

fn mcp_cassette_decide(key: &str) -> sema_core::McpCassetteDecision {
    use crate::cassette::Decision;
    CASSETTE.with(|c| match c.borrow().as_ref() {
        // No active cassette → behave as passthrough (real call, no recording).
        None => sema_core::McpCassetteDecision::Record,
        Some(cass) => match cass.decide(key) {
            Decision::Replay(entry) => match entry.mcp_result {
                Some(value) => sema_core::McpCassetteDecision::Replay(value),
                // Present under this key but not an mcp-call entry — treat as drift.
                None => sema_core::McpCassetteDecision::Miss,
            },
            Decision::Miss(_) => sema_core::McpCassetteDecision::Miss,
            Decision::Record => sema_core::McpCassetteDecision::Record,
        },
    })
}

fn mcp_cassette_record(key: &str, value: &serde_json::Value) {
    CASSETTE.with(|c| {
        if let Some(cass) = c.borrow_mut().as_mut() {
            cass.record_entry(crate::cassette::TapeEntry::from_mcp_call(key, value));
        }
    });
}

/// Install a cassette on the current thread (programmatic/test entry point;
/// the env path `SEMA_LLM_CASSETTE` uses the same thread-local).
pub fn install_cassette(cassette: crate::cassette::Cassette) {
    CASSETTE.with(|c| *c.borrow_mut() = Some(cassette));
}

/// Remove and return the active cassette (e.g. to flush it to disk after
/// recording).
pub fn take_cassette() -> Option<crate::cassette::Cassette> {
    CASSETTE.with(|c| c.borrow_mut().take())
}

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

fn register_fn(env: &Env, name: &str, f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::simple(name, f)),
    );
}

fn register_fn_ctx(
    env: &Env,
    name: &str,
    f: impl Fn(&EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::with_ctx(name, f)),
    );
}

fn with_provider<F, R>(f: F) -> Result<R, SemaError>
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

fn with_embedding_provider<F, R>(f: F) -> Result<R, SemaError>
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
fn metadata_text(metadata: &Value) -> String {
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

fn with_rerank_provider<F, R>(name: Option<&str>, f: F) -> Result<R, SemaError>
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

fn track_usage(usage: &Usage) -> Result<(), SemaError> {
    // Price the model as served by the provider that produced this response (falls back to
    // the canonical first-party price when the serving provider is unknown).
    let provider = take_serving_provider().unwrap_or_default();
    let cost = pricing::calculate_cost_for(&provider, usage);
    let total_tokens = (usage.prompt_tokens + usage.completion_tokens) as u64;

    LAST_USAGE.with(|u| *u.borrow_mut() = Some(usage.clone()));
    // Fold into the active per-task leaf accumulator for the workflow runtime. SUMS
    // every round of a multi-round tool loop; cache hits (all-zero) don't bump `calls`.
    // The async poller captures the leaf's own frame Rc and folds there instead, so it
    // sets USAGE_ACCUM_SUPPRESS to keep this fold from double-counting.
    if !USAGE_ACCUM_SUPPRESS.with(|s| s.get()) {
        if let Some(slot) = current_usage_accum() {
            accumulate_into(&slot, usage, cost);
        }
    }
    SESSION_USAGE.with(|u| {
        let mut session = u.borrow_mut();
        session.prompt_tokens += usage.prompt_tokens;
        session.completion_tokens += usage.completion_tokens;
        session.cache_read_input_tokens += usage.cache_read_input_tokens;
        session.cache_creation_input_tokens += usage.cache_creation_input_tokens;
    });

    // Session cost is a global accumulator, tracked independently of any budget scope.
    if let Some(c) = cost {
        SESSION_COST.with(|sc| *sc.borrow_mut() += c);
    }

    // Charge the active (per-task) budget frame and enforce its limits. Because the
    // frame is shared by `Rc`, all concurrent tasks spawned in one `with-budget` charge
    // one aggregate — so a fan-out is gated, not just a single sequential call.
    if let Some(frame) = active_budget() {
        charge_budget_frame(&frame, total_tokens, cost)?;
        // Cost unknown while a cost cap is set — warn once (enforcement is best-effort).
        if cost.is_none() && frame.borrow().cost_limit.is_some() {
            PRICING_WARNING_SHOWN.with(|shown| {
                if !shown.get() {
                    shown.set(true);
                    eprintln!(
                        "Warning: pricing unknown for model '{}'; budget enforcement is best-effort",
                        usage.model
                    );
                }
            });
        }
    }

    Ok(())
}

/// Clone the active (per-task) budget frame's `Rc`, if any. The sync path charges
/// this via `track_usage`; the async completion poller captures it at yield time and
/// re-installs it around its own `track_usage` so the charge lands on the frame that
/// was active when the completion was DISPATCHED, not whatever is active when the
/// future resolves.
fn active_budget() -> Option<Rc<RefCell<BudgetFrame>>> {
    ACTIVE_BUDGET.with(|b| b.borrow().clone())
}

/// Ensure an active budget frame exists (creating an unbounded one if none), returning
/// its `Rc`. Used by the non-scoped `llm/set-budget`/`llm/set-token-budget` API.
fn ensure_active_budget() -> Rc<RefCell<BudgetFrame>> {
    ACTIVE_BUDGET.with(|b| {
        b.borrow_mut()
            .get_or_insert_with(|| Rc::new(RefCell::new(BudgetFrame::default())))
            .clone()
    })
}

/// Charge `total_tokens` / `cost` into `frame` and return `Err` if either limit is now
/// exceeded. Cost is charged only when known (`Some`); the token charge always applies.
/// Shared by the sync path and the async poller so both gate identically.
fn charge_budget_frame(
    frame: &Rc<RefCell<BudgetFrame>>,
    total_tokens: u64,
    cost: Option<f64>,
) -> Result<(), SemaError> {
    let mut f = frame.borrow_mut();
    f.tokens_spent += total_tokens;
    if let Some(max_tokens) = f.token_limit {
        if f.tokens_spent > max_tokens {
            return Err(SemaError::Llm(format!(
                "token budget exceeded: used {} of {} tokens",
                f.tokens_spent, max_tokens
            )));
        }
    }
    if let Some(c) = cost {
        f.cost_spent += c;
        if let Some(max_cost) = f.cost_limit {
            if f.cost_spent > max_cost {
                return Err(SemaError::Llm(format!(
                    "budget exceeded: spent ${:.4} of ${:.4} limit",
                    f.cost_spent, max_cost
                )));
            }
        }
    }
    Ok(())
}

/// Set a cost budget limit for LLM calls (non-scoped API; mutates the active frame).
pub fn set_budget(max_cost_usd: f64) {
    let frame = ensure_active_budget();
    let mut f = frame.borrow_mut();
    f.cost_limit = Some(max_cost_usd);
    f.cost_spent = 0.0;
}

/// Set a token budget limit for LLM calls (non-scoped API; mutates the active frame).
pub fn set_token_budget(max_tokens: u64) {
    let frame = ensure_active_budget();
    let mut f = frame.borrow_mut();
    f.token_limit = Some(max_tokens);
    f.tokens_spent = 0;
}

/// Clear the budget limits on the active frame.
pub fn clear_budget() {
    if let Some(frame) = active_budget() {
        let mut f = frame.borrow_mut();
        f.cost_limit = None;
        f.token_limit = None;
    }
}

/// Push a scoped budget: save the current active frame and install a FRESH one (spent
/// reset to zero) for the new scope. Concurrent tasks spawned inside this scope capture
/// the fresh frame's `Rc` and charge it as one aggregate.
pub fn push_budget_scope(max_cost_usd: Option<f64>, max_tokens: Option<u64>) {
    let prev = ACTIVE_BUDGET.with(|b| b.borrow().clone());
    BUDGET_STACK.with(|stack| stack.borrow_mut().push(prev));
    let frame = BudgetFrame {
        cost_limit: max_cost_usd,
        cost_spent: 0.0,
        token_limit: max_tokens,
        tokens_spent: 0,
    };
    ACTIVE_BUDGET.with(|b| *b.borrow_mut() = Some(Rc::new(RefCell::new(frame))));
}

/// Pop a scoped budget and restore the previously-active frame (`None` at the outermost).
pub fn pop_budget_scope() {
    let prev = BUDGET_STACK
        .with(|stack| stack.borrow_mut().pop())
        .flatten();
    ACTIVE_BUDGET.with(|b| *b.borrow_mut() = prev);
}

fn get_opt_string(opts: &BTreeMap<Value, Value>, key: &str) -> Option<String> {
    opts.get(&Value::keyword(key))
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

/// Read an option that may be given as a keyword (`:high`) or a string
/// (`"high"`) — used for `:reasoning-effort`.
fn get_opt_effort(opts: &BTreeMap<Value, Value>, key: &str) -> Option<String> {
    opts.get(&Value::keyword(key))
        .and_then(|v| v.as_keyword().or_else(|| v.as_str().map(|s| s.to_string())))
}

/// Parse one `llm/with-fallback` chain element into a [`FallbackEntry`].
///
/// Accepted shapes:
/// - `:provider` / `"provider"` — bare name, uses the provider's default model
/// - `[:provider "model"]` — pair, with a per-provider model override
/// - `{:provider :name :model "model"}` — map form, `:model` optional
fn parse_fallback_entry(v: &Value) -> Result<FallbackEntry, SemaError> {
    // Bare keyword or string.
    if let Some(name) = v.as_keyword().or_else(|| v.as_str().map(|s| s.to_string())) {
        return Ok(FallbackEntry {
            provider: name,
            model: None,
        });
    }
    // Map form: {:provider .. :model ..}. The :provider value may be a keyword or
    // a string.
    if let Some(map) = v.as_map_ref() {
        let provider = map
            .get(&Value::keyword("provider"))
            .and_then(|p| p.as_keyword().or_else(|| p.as_str().map(|s| s.to_string())))
            .ok_or_else(|| {
                SemaError::eval("fallback map entry must have a :provider key (keyword or string)")
            })?;
        return Ok(FallbackEntry {
            provider,
            model: get_opt_string(map, "model"),
        });
    }
    // Pair form: [:provider "model"].
    if let Some(seq) = v.as_seq() {
        if seq.len() != 2 {
            return Err(SemaError::eval(
                "fallback pair entry must be [provider model]",
            ));
        }
        let provider = seq[0]
            .as_keyword()
            .or_else(|| seq[0].as_str().map(|s| s.to_string()))
            .ok_or_else(|| SemaError::type_error("keyword or string", seq[0].type_name()))?;
        let model = seq[1]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| SemaError::type_error("string model", seq[1].type_name()))?;
        return Ok(FallbackEntry {
            provider,
            model: Some(model),
        });
    }
    Err(SemaError::type_error(
        "keyword, string, [provider model] pair, or map",
        v.type_name(),
    ))
}

fn get_opt_f64(opts: &BTreeMap<Value, Value>, key: &str) -> Option<f64> {
    opts.get(&Value::keyword(key)).and_then(|v| v.as_float())
}

fn get_opt_u32(opts: &BTreeMap<Value, Value>, key: &str) -> Option<u32> {
    opts.get(&Value::keyword(key))
        .and_then(|v| v.as_int())
        .map(|n| n as u32)
}

/// Read an optional per-call `:timeout` (milliseconds) from a call's options argument.
fn opt_timeout_ms(opts_arg: Option<&Value>) -> Option<u64> {
    opts_arg
        .and_then(|v| v.as_map_rc())
        .and_then(|o| get_opt_u32(&o, "timeout"))
        .map(|n| n as u64)
}

/// Read an optional list-of-strings option for observability tags: `:tags ["a" "b"]`,
/// or a lone string `:tags "a"`. Non-string elements are skipped.
fn get_opt_string_list(opts: &BTreeMap<Value, Value>, key: &str) -> Vec<String> {
    match opts.get(&Value::keyword(key)) {
        Some(v) if v.as_seq().is_some() => v
            .as_seq()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()).or_else(|| x.as_keyword()))
            .collect(),
        Some(v) => v
            .as_str()
            .map(|s| vec![s.to_string()])
            .or_else(|| v.as_keyword().map(|s| vec![s]))
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Read an optional `string -> string` map option for observability metadata:
/// `:metadata {:env "prod" :team "ml"}`. Keyword keys are de-coloned (`:env` -> `env`);
/// values are stringified.
fn get_opt_str_map(opts: &BTreeMap<Value, Value>, key: &str) -> Vec<(String, String)> {
    let Some(m) = opts.get(&Value::keyword(key)).and_then(|v| v.as_map_rc()) else {
        return Vec::new();
    };
    m.iter()
        .map(|(k, val)| {
            let ks = k
                .as_keyword()
                .or_else(|| k.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| k.to_string());
            let vs = val
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| val.to_string());
            (ks, vs)
        })
        .collect()
}

/// Substitute `{{key}}` placeholders in a template string using a vars map.
/// Keys are looked up as keywords in the map. Unfilled slots are left as-is.
fn fill_template(template: &str, vars: &BTreeMap<Value, Value>) -> String {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'{') {
            chars.next();
            let mut var_name = String::new();
            let mut found_close = false;
            while let Some(c) = chars.next() {
                if c == '}' && chars.peek() == Some(&'}') {
                    chars.next();
                    found_close = true;
                    break;
                }
                var_name.push(c);
            }
            if found_close {
                if let Some(val) = vars.get(&Value::keyword(&var_name)) {
                    if let Some(s) = val.as_str() {
                        result.push_str(s);
                    } else {
                        result.push_str(&val.to_string());
                    }
                } else {
                    result.push_str("{{");
                    result.push_str(&var_name);
                    result.push_str("}}");
                }
            } else {
                result.push_str("{{");
                result.push_str(&var_name);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Parse a message role keyword (`:system`/`:user`/`:assistant`/`:tool`) for the
/// conversation-surgery builtins, erroring with `who` in the message on anything else.
fn parse_role(v: &Value, who: &str) -> Result<Role, SemaError> {
    let kw = v
        .as_keyword()
        .ok_or_else(|| SemaError::type_error("keyword", v.type_name()))?;
    match kw.as_str() {
        "system" => Ok(Role::System),
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        "tool" => Ok(Role::Tool),
        other => Err(SemaError::eval(format!("{who}: unknown role '{other}'"))),
    }
}

/// Build a `Message` from the tail of a surgery call: either a single message value
/// (`(op conv i msg)`) or a `role`/`content` pair (`(op conv i :system "…")`).
fn message_from_tail(tail: &[Value], who: &str) -> Result<Message, SemaError> {
    match tail {
        [m] => m
            .as_message_rc()
            .map(|rc| (*rc).clone())
            .ok_or_else(|| SemaError::type_error("message", m.type_name())),
        [role, content] => Ok(Message {
            role: parse_role(role, who)?,
            content: content
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| content.to_string()),
            images: Vec::new(),
        }),
        _ => Err(SemaError::arity(who, "3-4", tail.len() + 2)),
    }
}

/// Identity key for prompt-algebra dedup/compare: two messages are "the same" when
/// role and content match (images are ignored).
fn msg_key(m: &Message) -> (Role, &str) {
    (m.role.clone(), m.content.as_str())
}

/// Fold a completed turn's real `usage` into a conversation's metadata so that
/// `conversation/cost`/`conversation/stats` report actual billed figures. Cost is only
/// accumulated when the model's price is known; if no turn ever contributes a priced
/// usage, `usage-cost` stays absent and `conversation/cost` returns nil.
fn accumulate_usage(meta: &mut BTreeMap<String, String>, usage: &Usage) {
    let add_u32 = |meta: &mut BTreeMap<String, String>, key: &str, delta: u32| {
        let prev: u64 = meta.get(key).and_then(|s| s.parse().ok()).unwrap_or(0);
        meta.insert(key.to_string(), (prev + delta as u64).to_string());
    };
    add_u32(meta, "usage-prompt-tokens", usage.prompt_tokens);
    add_u32(meta, "usage-completion-tokens", usage.completion_tokens);
    if let Some(cost) = pricing::calculate_cost(usage) {
        let prev: f64 = meta
            .get("usage-cost")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        meta.insert("usage-cost".to_string(), (prev + cost).to_string());
    }
}

/// A provider defined in Sema code via lambdas.
/// Only stores String fields (Send+Sync); callbacks live in the
/// LISP_PROVIDERS thread-local, accessed only from the same thread.
struct LispProvider {
    name: String,
    default_model: String,
}

impl LlmProvider for LispProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let name = self.name.clone();
        LISP_PROVIDERS.with(|providers| {
            let providers = providers.borrow();
            let callbacks = providers.get(&name).ok_or_else(|| {
                LlmError::Config(format!("lisp provider '{}' callbacks not found", name))
            })?;
            let complete_fn = callbacks.complete_fn.clone();

            let request_map = chat_request_to_value(&request);

            // The LlmProvider trait gives us no caller ctx, so invoke the user's
            // `:complete` function on the shared stdlib context, which carries the
            // registered evaluator callback (same path stdlib HOFs use).
            let result = sema_core::with_stdlib_ctx(|ctx| {
                sema_core::call_callback(ctx, &complete_fn, &[request_map])
            });

            match result {
                Ok(response_val) => parse_lisp_provider_response(&response_val, &request.model),
                Err(e) => Err(LlmError::Api {
                    status: 0,
                    message: e.to_string(),
                }),
            }
        })
    }
}

/// Convert a ChatRequest into a Sema Value::Map for passing to Lisp provider callbacks.
fn chat_request_to_value(request: &ChatRequest) -> Value {
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("model"), Value::string(&request.model));

    let msgs: Vec<Value> = request
        .messages
        .iter()
        .map(|m| {
            let mut msg_map = BTreeMap::new();
            msg_map.insert(Value::keyword("role"), Value::string(&m.role));
            msg_map.insert(
                Value::keyword("content"),
                Value::string(&m.content.to_text()),
            );
            Value::map(msg_map)
        })
        .collect();
    map.insert(Value::keyword("messages"), Value::list(msgs));

    if let Some(max_tokens) = request.max_tokens {
        map.insert(Value::keyword("max-tokens"), Value::int(max_tokens as i64));
    }
    if let Some(temp) = request.temperature {
        map.insert(Value::keyword("temperature"), Value::float(temp));
    }
    if let Some(ref system) = request.system {
        map.insert(Value::keyword("system"), Value::string(system));
    }

    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                let mut tool_map = BTreeMap::new();
                tool_map.insert(Value::keyword("name"), Value::string(&t.name));
                tool_map.insert(Value::keyword("description"), Value::string(&t.description));
                tool_map.insert(
                    Value::keyword("parameters"),
                    sema_core::json_to_value(&t.parameters),
                );
                Value::map(tool_map)
            })
            .collect();
        map.insert(Value::keyword("tools"), Value::list(tools));
    }

    if !request.stop_sequences.is_empty() {
        let seqs: Vec<Value> = request
            .stop_sequences
            .iter()
            .map(|s| Value::string(s))
            .collect();
        map.insert(Value::keyword("stop-sequences"), Value::list(seqs));
    }

    Value::map(map)
}

/// Parse a Sema Value returned by a Lisp provider callback into a ChatResponse.
fn parse_lisp_provider_response(val: &Value, model: &str) -> Result<ChatResponse, LlmError> {
    match val.view() {
        ValueView::String(s) => Ok(ChatResponse {
            content: s.to_string(),
            role: "assistant".to_string(),
            model: model.to_string(),
            tool_calls: vec![],
            usage: Usage::default(),
            stop_reason: Some("end_turn".to_string()),
        }),
        ValueView::Map(map) => {
            let content = map
                .get(&Value::keyword("content"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let role = map
                .get(&Value::keyword("role"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "assistant".to_string());
            let resp_model = map
                .get(&Value::keyword("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| model.to_string());
            let stop_reason = map
                .get(&Value::keyword("stop-reason"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or(Some("end_turn".to_string()));

            let usage = if let Some(usage_val) = map.get(&Value::keyword("usage")) {
                if let Some(usage_map) = usage_val.as_map_rc() {
                    let prompt_tokens = usage_map
                        .get(&Value::keyword("prompt-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    let completion_tokens = usage_map
                        .get(&Value::keyword("completion-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    let cache_read_input_tokens = usage_map
                        .get(&Value::keyword("cache-read-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    let cache_creation_input_tokens = usage_map
                        .get(&Value::keyword("cache-creation-tokens"))
                        .and_then(|v| v.as_int())
                        .unwrap_or(0) as u32;
                    Usage {
                        prompt_tokens,
                        completion_tokens,
                        model: resp_model.clone(),
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    }
                } else {
                    Usage {
                        model: resp_model.clone(),
                        ..Default::default()
                    }
                }
            } else {
                Usage {
                    model: resp_model.clone(),
                    ..Default::default()
                }
            };

            let tool_calls = if let Some(tcs_val) = map.get(&Value::keyword("tool-calls")) {
                if let Some(tcs) = tcs_val.as_seq() {
                    tcs.iter()
                        .filter_map(|tc| {
                            let tc_map = tc.as_map_rc()?;
                            let id = tc_map
                                .get(&Value::keyword("id"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .unwrap_or_default();
                            let name = tc_map
                                .get(&Value::keyword("name"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())?;
                            let arguments = tc_map
                                .get(&Value::keyword("arguments"))
                                .map(sema_core::value_to_json_lossy)
                                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                            Some(ToolCall {
                                id,
                                name,
                                arguments,
                                thought_signature: tc_map
                                    .get(&Value::keyword("thought-signature"))
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                            })
                        })
                        .collect()
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

            Ok(ChatResponse {
                content,
                role,
                model: resp_model,
                tool_calls,
                usage,
                stop_reason,
            })
        }
        _ => Err(LlmError::Parse(
            "lisp provider must return a string or map with :content".to_string(),
        )),
    }
}

fn register_fn_gated(
    env: &Env,
    sandbox: &sema_core::Sandbox,
    cap: sema_core::Caps,
    name: &str,
    f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static,
) {
    if sandbox.is_unrestricted() {
        register_fn(env, name, f);
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        register_fn(env, name, move |args| {
            sandbox.check(cap, &fn_name)?;
            f(args)
        });
    }
}

fn register_fn_ctx_gated(
    env: &Env,
    sandbox: &sema_core::Sandbox,
    cap: sema_core::Caps,
    name: &str,
    f: impl Fn(&sema_core::EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
) {
    if sandbox.is_unrestricted() {
        register_fn_ctx(env, name, f);
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        register_fn_ctx(env, name, move |ctx, args| {
            sandbox.check(cap, &fn_name)?;
            f(ctx, args)
        });
    }
}

/// Extract the host from a provider `base-url`/`host` string without pulling in
/// a URL-parsing dependency. Handles `scheme://`, userinfo, `[ipv6]`, and ports.
fn url_host(url: &str) -> Option<String> {
    let after = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = after.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(rest) = hostport.strip_prefix('[') {
        // [::1]:port -> ::1
        return rest.split(']').next().map(|s| s.to_string());
    }
    hostport.split(':').next().map(|s| s.to_string())
}

/// True if `host` points at the local machine or a private/internal network —
/// the targets an SSRF would pivot to. Used to reject attacker-chosen provider
/// `base-url`s when running untrusted (sandboxed) code.
fn is_internal_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    if h.is_empty() || h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => ipv4_is_internal(v4),
        Ok(std::net::IpAddr::V6(v6)) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            // IPv4-mapped (::ffff:a.b.c.d) — re-check against v4 rules.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ipv4_is_internal(v4);
            }
            let seg0 = v6.segments()[0];
            (seg0 & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (seg0 & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
        // `IpAddr::parse` only accepts canonical dotted-decimal, but
        // `getaddrinfo` (what reqwest ultimately calls) also accepts the
        // inet_aton forms: decimal (`2130706433`), octal (`0177.0.0.1`),
        // hex (`0x7f.0.0.1`), and short (`127.1`). Decode those and re-check,
        // so e.g. `http://2130706433/` can't smuggle loopback past the gate.
        Err(_) => parse_loose_ipv4(&h).map(ipv4_is_internal).unwrap_or(false),
    }
}

/// Internal/private/loopback test shared by every IPv4 path.
fn ipv4_is_internal(v4: std::net::Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.octets()[0] == 0
}

/// Parse the loose `inet_aton` IPv4 forms that `getaddrinfo` accepts but
/// `Ipv4Addr::from_str` rejects: 1–4 dot-separated parts, each decimal,
/// octal (`0` prefix), or hex (`0x` prefix); a trailing dot is allowed.
/// Returns `None` for anything that isn't such a numeric address (i.e. a real
/// hostname), so non-numeric public hosts fall through to "not internal".
fn parse_loose_ipv4(host: &str) -> Option<std::net::Ipv4Addr> {
    let host = host.strip_suffix('.').unwrap_or(host);
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let nums: Vec<u32> = parts
        .iter()
        .map(|p| parse_uint_part(p))
        .collect::<Option<Vec<_>>>()?;
    let addr: u32 = match nums.as_slice() {
        [a] => *a,
        [a, b] if *a <= 0xff && *b <= 0x00ff_ffff => (a << 24) | b,
        [a, b, c] if *a <= 0xff && *b <= 0xff && *c <= 0xffff => (a << 24) | (b << 16) | c,
        [a, b, c, d] if [a, b, c, d].iter().all(|x| **x <= 0xff) => {
            (a << 24) | (b << 16) | (c << 8) | d
        }
        _ => return None, // a part overflowed its field — not a valid packed address
    };
    Some(std::net::Ipv4Addr::from(addr))
}

/// Parse a single inet_aton numeric part: hex (`0x..`), octal (`0..`), decimal.
fn parse_uint_part(s: &str) -> Option<u32> {
    if let Some(hex) = s.strip_prefix("0x") {
        if hex.is_empty() {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()
    } else if s.len() > 1 && s.starts_with('0') {
        u32::from_str_radix(&s[1..], 8).ok()
    } else {
        s.parse::<u32>().ok()
    }
}

/// Reject provider URLs that target internal hosts when running sandboxed.
/// Trusted (unrestricted) sessions — the normal CLI/REPL/notebook — keep full
/// access so local proxies and Ollama on `localhost` continue to work.
fn guard_provider_url(unrestricted: bool, opts: &BTreeMap<Value, Value>) -> Result<(), SemaError> {
    if unrestricted {
        return Ok(());
    }
    let url = get_opt_string(opts, "base-url").or_else(|| get_opt_string(opts, "host"));
    if let Some(url) = url {
        if url_host(&url).is_some_and(|h| is_internal_host(&h)) {
            return Err(SemaError::eval(format!(
                "llm/configure: base-url '{url}' targets an internal/loopback host, \
                 which is not allowed under the current sandbox"
            ))
            .with_hint(
                "grant the network capability and run unsandboxed to use a local endpoint",
            ));
        }
    }
    Ok(())
}

/// Cycle-collector pass observer: forwards each pass to sema-otel as a
/// `gc.collect` span (no-op while telemetry is disabled). A plain `fn`, so it
/// cannot capture `Value`/`Env` state; it touches no Sema heap.
fn gc_otel_observer(event: &sema_core::GcPassEvent) {
    sema_otel::gc_pass_span(event);
}

pub fn register_llm_builtins(env: &Env, sandbox: &sema_core::Sandbox) {
    let unrestricted = sandbox.is_unrestricted();

    // Install THE process-wide I/O pool behind the sema-core executor seam
    // (ADR #69). Idempotent, first-wins.
    sema_io::install();

    // Wire the per-task otel context-swap callbacks into sema-core so the
    // cooperative scheduler (sema-vm, which can't depend on sema-otel) can swap
    // the otel span stack + ids on task-switch. Idempotent (just resets two
    // thread-local fn pointers); registering here keeps it in a crate that names
    // both `sema_core` and `sema_otel`.
    sema_otel::register_task_callbacks();

    // Cycle-collector observability: every collector pass that actually runs
    // (any trigger, aborted included) emits a retroactively-timed `gc.collect`
    // span, so GC work shows up on the same timeline as LLM/tool spans. Same
    // seam as above — sema-core can't depend on sema-otel, and the observer is
    // a plain `fn` (captures nothing; invariant I2). Idempotent.
    sema_core::set_gc_observer(Some(gc_otel_observer));

    // Bridge the LLM cassette to MCP tool calls (sema-mcp consults this via
    // sema-core). Idempotent — just sets two thread-local fn pointers.
    sema_core::set_mcp_cassette_hook(mcp_cassette_decide, mcp_cassette_record);

    // Reclaim non-blocking agent-run slab entries owned by a CANCELLED task
    // (whose `__agent-finish` can never run) the moment the scheduler reaps it —
    // ending the agent span balanced on the VM thread instead of leaking the
    // entry (and its telemetry) until `reset_runtime_state`. Same type-erased
    // fn-pointer seam as the callbacks above; idempotent. Invariant I2 holds:
    // a plain `fn`, captures nothing.
    sema_core::set_task_reaped_callback(reap_cancelled_agent_runs);

    // CI/global cassette: SEMA_LLM_CASSETTE=path [+ SEMA_LLM_CASSETTE_MODE=replay|
    // record|auto] installs a cassette for the whole process, so a suite can be
    // forced into deterministic replay without touching test source. Only honored
    // outside the sandbox (it reads/writes a file path from the environment).
    if unrestricted {
        if let Ok(path) = std::env::var("SEMA_LLM_CASSETTE") {
            if !path.is_empty() {
                let mode = std::env::var("SEMA_LLM_CASSETTE_MODE")
                    .map(|s| crate::cassette::CassetteMode::parse(&s))
                    .unwrap_or(crate::cassette::CassetteMode::Auto);
                let cassette =
                    crate::cassette::Cassette::load(std::path::PathBuf::from(path), mode);
                CASSETTE.with(|c| *c.borrow_mut() = Some(cassette));
            }
        }
    }
    // (llm/configure :anthropic {:api-key "..." :default-model "..."})
    // (llm/configure :openai {:api-key "..." :base-url "..." :default-model "..."})
    register_fn(env, "llm/configure", move |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/configure", "2", args.len()));
        }
        let provider_name = args[0]
            .as_keyword()
            .ok_or_else(|| SemaError::type_error("keyword", args[0].type_name()))?;
        let opts_rc = args[1]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        let opts = opts_rc.as_ref().clone();

        guard_provider_url(unrestricted, &opts)?;

        let api_key = get_opt_string(&opts, "api-key");

        PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            match provider_name.as_str() {
                "anthropic" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model");
                    let provider = AnthropicProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("anthropic");
                }
                "openai" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let base_url = get_opt_string(&opts, "base-url");
                    let model = get_opt_string(&opts, "default-model");
                    let provider = OpenAiProvider::new(api_key, base_url, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("openai");
                }
                "gemini" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model");
                    let provider = GeminiProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("gemini");
                }
                "groq" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "llama-3.3-70b-versatile".to_string());
                    let base_url = get_opt_string(&opts, "base-url")
                        .unwrap_or_else(|| "https://api.groq.com/openai/v1".to_string());
                    let provider =
                        OpenAiProvider::named("groq".to_string(), api_key, base_url, model, true)
                            .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("groq");
                }
                "xai" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "grok-4.3".to_string());
                    let base_url = get_opt_string(&opts, "base-url")
                        .unwrap_or_else(|| "https://api.x.ai/v1".to_string());
                    let provider =
                        OpenAiProvider::named("xai".to_string(), api_key, base_url, model, true)
                            .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("xai");
                }
                "mistral" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "mistral-large-latest".to_string());
                    let base_url = get_opt_string(&opts, "base-url")
                        .unwrap_or_else(|| "https://api.mistral.ai/v1".to_string());
                    let provider = OpenAiProvider::named(
                        "mistral".to_string(),
                        api_key,
                        base_url,
                        model,
                        false,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("mistral");
                }
                "moonshot" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "kimi-k2.6".to_string());
                    let base_url = get_opt_string(&opts, "base-url")
                        .unwrap_or_else(|| "https://api.moonshot.ai/v1".to_string());
                    let provider = OpenAiProvider::named(
                        "moonshot".to_string(),
                        api_key,
                        base_url,
                        model,
                        false,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("moonshot");
                }
                "ollama" => {
                    let host =
                        get_opt_string(&opts, "host").or_else(|| get_opt_string(&opts, "base-url"));
                    let model = get_opt_string(&opts, "default-model");
                    // Ollama doesn't use api-key
                    let provider = OllamaProvider::new(host, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default("ollama");
                }
                "jina" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "jina-embeddings-v3".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "jina".to_string(),
                        api_key,
                        "https://api.jina.ai/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?
                    .with_rerank(crate::embeddings::RerankDialect::Jina);
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("jina");
                    reg.set_rerank_provider("jina");
                }
                "voyage" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "voyage-3-lite".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "voyage".to_string(),
                        api_key,
                        "https://api.voyageai.com/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?
                    .with_rerank(crate::embeddings::RerankDialect::Voyage);
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("voyage");
                    reg.set_rerank_provider("voyage");
                }
                "cohere" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model");
                    let provider = CohereEmbeddingProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("cohere");
                    reg.set_rerank_provider("cohere");
                }
                other => {
                    // Treat unknown providers as OpenAI-compatible if base-url and api-key are provided
                    let api_key = api_key.clone().ok_or_else(|| {
                        SemaError::Llm(format!(
                            "unknown provider '{other}': provide :api-key and :base-url to register as OpenAI-compatible"
                        ))
                    })?;
                    let base_url = get_opt_string(&opts, "base-url").ok_or_else(|| {
                        SemaError::Llm(format!(
                            "unknown provider '{other}': provide :base-url to register as OpenAI-compatible"
                        ))
                    })?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "default".to_string());
                    let provider = OpenAiProvider::named(
                        other.to_string(),
                        api_key,
                        base_url,
                        model,
                        false,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_default(other);
                }
            }
            Ok(Value::nil())
        })
    });

    // (llm/define-provider :name {:complete fn :default-model "..." :stream fn})
    register_fn(env, "llm/define-provider", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/define-provider", "2", args.len()));
        }
        let provider_name = args[0]
            .as_keyword()
            .ok_or_else(|| SemaError::type_error("keyword", args[0].type_name()))?;
        let opts_rc = args[1]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        let opts = opts_rc.as_ref().clone();

        let complete_fn = opts
            .get(&Value::keyword("complete"))
            .cloned()
            .ok_or_else(|| SemaError::eval("llm/define-provider requires :complete function"))?;

        if complete_fn.as_lambda_rc().is_none() && complete_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", complete_fn.type_name()));
        }

        let default_model =
            get_opt_string(&opts, "default-model").unwrap_or_else(|| "default".to_string());

        let name_for_callbacks = provider_name.clone();
        LISP_PROVIDERS.with(|providers| {
            providers
                .borrow_mut()
                .insert(name_for_callbacks, LispProviderCallbacks { complete_fn });
        });

        let name_for_registry = provider_name.clone();
        let model_clone = default_model.clone();
        PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            reg.register(Box::new(LispProvider {
                name: name_for_registry,
                default_model: model_clone,
            }));
            reg.set_default(&provider_name);
        });

        Ok(Value::keyword(&provider_name))
    });

    // Auto-configure from environment variables
    register_fn(env, "llm/auto-configure", |_args| {
        // New scoped env vars (preferred)
        let chat_model = std::env::var("SEMA_CHAT_MODEL")
            .ok()
            .filter(|m| !m.is_empty());
        let chat_provider = std::env::var("SEMA_CHAT_PROVIDER")
            .ok()
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty());
        let embedding_model = std::env::var("SEMA_EMBEDDING_MODEL")
            .ok()
            .filter(|m| !m.is_empty());
        let embedding_provider = std::env::var("SEMA_EMBEDDING_PROVIDER")
            .ok()
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty());

        let forced_chat_model = chat_model;
        let forced_chat_provider = chat_provider;

        let result = PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            let mut first_configured: Option<String> = None;

            // Determine which provider gets the chat model override.
            // Only the provider that will become the default chat provider should
            // receive forced_chat_model — not every provider.
            let target_chat_provider = forced_chat_provider.as_deref();

            // Inline to avoid borrow conflicts with first_configured.
            macro_rules! model_for {
                ($name:expr) => {{
                    match target_chat_provider {
                        Some(target) if target == $name => forced_chat_model.clone(),
                        None if first_configured.is_none() => forced_chat_model.clone(),
                        _ => None,
                    }
                }};
            }

            // Try Anthropic first (preferred)
            if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                if !key.is_empty() {
                    let provider = AnthropicProvider::new(key, model_for!("anthropic"))
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("anthropic");
                        first_configured = Some("anthropic".to_string());
                    }
                }
            }
            // Try OpenAI
            if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                if !key.is_empty() {
                    let provider = OpenAiProvider::new(key, None, model_for!("openai"))
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("openai");
                        first_configured = Some("openai".to_string());
                    }
                }
            }
            // Try Groq
            if let Ok(key) = std::env::var("GROQ_API_KEY") {
                if !key.is_empty() {
                    let model =
                        model_for!("groq").unwrap_or_else(|| "llama-3.3-70b-versatile".to_string());
                    let provider = OpenAiProvider::named(
                        "groq".to_string(),
                        key,
                        "https://api.groq.com/openai/v1".to_string(),
                        model,
                        true,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("groq");
                        first_configured = Some("groq".to_string());
                    }
                }
            }
            // Try xAI
            if let Ok(key) = std::env::var("XAI_API_KEY") {
                if !key.is_empty() {
                    let model = model_for!("xai").unwrap_or_else(|| "grok-4.3".to_string());
                    let provider = OpenAiProvider::named(
                        "xai".to_string(),
                        key,
                        "https://api.x.ai/v1".to_string(),
                        model,
                        true,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("xai");
                        first_configured = Some("xai".to_string());
                    }
                }
            }
            // Try Mistral
            if let Ok(key) = std::env::var("MISTRAL_API_KEY") {
                if !key.is_empty() {
                    let model =
                        model_for!("mistral").unwrap_or_else(|| "mistral-large-latest".to_string());
                    let provider = OpenAiProvider::named(
                        "mistral".to_string(),
                        key,
                        "https://api.mistral.ai/v1".to_string(),
                        model,
                        false,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("mistral");
                        first_configured = Some("mistral".to_string());
                    }
                }
            }
            // Try Moonshot
            if let Ok(key) = std::env::var("MOONSHOT_API_KEY") {
                if !key.is_empty() {
                    let model = model_for!("moonshot").unwrap_or_else(|| "kimi-k2.6".to_string());
                    let provider = OpenAiProvider::named(
                        "moonshot".to_string(),
                        key,
                        "https://api.moonshot.ai/v1".to_string(),
                        model,
                        false,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("moonshot");
                        first_configured = Some("moonshot".to_string());
                    }
                }
            }
            // Try Google Gemini
            if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
                if !key.is_empty() {
                    let provider = GeminiProvider::new(key, model_for!("gemini"))
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if first_configured.is_none() {
                        reg.set_default("gemini");
                        first_configured = Some("gemini".to_string());
                    }
                }
            }
            // Ollama (local, no auth) — always register; defaults to http://localhost:11434
            {
                let provider = OllamaProvider::new(None, model_for!("ollama"))
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                reg.register(Box::new(provider));
                if first_configured.is_none() {
                    reg.set_default("ollama");
                    first_configured = Some("ollama".to_string());
                }
            }

            // Auto-configure embedding providers
            // Determine the embedding model override for the target embedding provider.
            // If --embedding-provider is set, only that provider gets the model override.
            // Otherwise, the first successfully configured embedding provider gets it.
            let target_embed_provider = embedding_provider.as_deref();

            // Helper: should this embedding provider get the model override?
            // Inline to avoid borrow conflicts with reg.
            macro_rules! embed_model_for {
                ($name:expr, $default:expr) => {{
                    let model_override = match target_embed_provider {
                        Some(target) if target == $name => embedding_model.clone(),
                        None if reg.embedding_provider().is_none() => embedding_model.clone(),
                        _ => None,
                    };
                    model_override.unwrap_or_else(|| $default.to_string())
                }};
            }

            if let Ok(key) = std::env::var("JINA_API_KEY") {
                if !key.is_empty() {
                    let model = embed_model_for!("jina", "jina-embeddings-v3");
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "jina".to_string(),
                        key,
                        "https://api.jina.ai/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?
                    .with_rerank(crate::embeddings::RerankDialect::Jina);
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("jina");
                    reg.set_rerank_provider("jina");
                }
            }
            if let Ok(key) = std::env::var("VOYAGE_API_KEY") {
                if !key.is_empty() {
                    let model = embed_model_for!("voyage", "voyage-3");
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "voyage".to_string(),
                        key,
                        "https://api.voyageai.com/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?
                    .with_rerank(crate::embeddings::RerankDialect::Voyage);
                    reg.register(Box::new(provider));
                    // Only set as embedding provider if not already set
                    if reg.embedding_provider().is_none() {
                        reg.set_embedding_provider("voyage");
                    }
                    if reg.rerank_provider().is_none() {
                        reg.set_rerank_provider("voyage");
                    }
                }
            }
            if let Ok(key) = std::env::var("COHERE_API_KEY") {
                if !key.is_empty() {
                    let model_override = match target_embed_provider {
                        Some("cohere") => embedding_model.clone(),
                        None if reg.embedding_provider().is_none() => embedding_model.clone(),
                        _ => None,
                    };
                    let provider = CohereEmbeddingProvider::new(key, model_override)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    if reg.embedding_provider().is_none() {
                        reg.set_embedding_provider("cohere");
                    }
                    if reg.rerank_provider().is_none() {
                        reg.set_rerank_provider("cohere");
                    }
                }
            }
            // Fallback: use OpenAI for embeddings if no dedicated provider was configured.
            // Use a distinct name to avoid overwriting the OpenAI chat provider.
            if reg.embedding_provider().is_none() {
                if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                    if !key.is_empty() {
                        let model = embed_model_for!("openai", "text-embedding-3-small");
                        let provider = OpenAiCompatEmbeddingProvider::new(
                            "openai-embeddings".to_string(),
                            key,
                            "https://api.openai.com/v1".to_string(),
                            model,
                        )
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                        reg.register(Box::new(provider));
                        reg.set_embedding_provider("openai-embeddings");
                    }
                }
            }

            // Apply forced chat provider override
            if let Some(requested_provider) = forced_chat_provider.as_deref() {
                if reg.get(requested_provider).is_some() {
                    reg.set_default(requested_provider);
                    first_configured = Some(requested_provider.to_string());
                } else {
                    return Err(SemaError::Llm(format!(
                        "requested provider is not configured: {requested_provider}"
                    )));
                }
            }

            // Apply forced embedding provider override
            if let Some(requested_embed) = target_embed_provider {
                if reg.get(requested_embed).is_some() {
                    reg.set_embedding_provider(requested_embed);
                } else {
                    return Err(SemaError::Llm(format!(
                        "requested embedding provider is not configured: {requested_embed}"
                    )));
                }
            }

            match first_configured {
                Some(name) => Ok(Value::keyword(&name)),
                None => Ok(Value::nil()),
            }
        })?;

        Ok(result)
    });

    // (llm/complete "prompt text" {:model "..." :max-tokens 200 :temperature 0.5})
    register_fn_gated(env, sandbox, sema_core::Caps::LLM, "llm/complete", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/complete", "1-2", args.len()));
        }
        let prompt_text = if let Some(p) = args[0].as_prompt_rc() {
            return complete_with_prompt(&p, args.get(1));
        } else if let Some(s) = args[0].as_str() {
            s.to_string()
        } else {
            return Err(SemaError::type_error(
                "string or prompt",
                args[0].type_name(),
            ));
        };

        let mut model = String::new();
        let mut max_tokens = None;
        let mut temperature = None;
        let mut system = None;
        let mut reasoning_effort = None;
        let mut conv_scope = ConvScope::default();

        if let Some(opts_val) = args.get(1) {
            if let Some(opts) = opts_val.as_map_rc() {
                conv_scope = ConvScope::from_opts(Some(&opts));
                model = get_opt_string(&opts, "model").unwrap_or_default();
                max_tokens = get_opt_u32(&opts, "max-tokens");
                temperature = get_opt_f64(&opts, "temperature");
                system = get_opt_string(&opts, "system");
                reasoning_effort = get_opt_effort(&opts, "reasoning-effort");
            }
        }

        // Honor a caller-supplied conversation/session/user identity (else do_complete
        // generates a fresh conversation id).
        let _conv = conv_scope.open();
        // Per-call observability tags/metadata (read inside do_complete's span).
        let _tele = install_call_telemetry(args.get(1).and_then(|v| v.as_map_rc()).as_ref());

        let messages = vec![ChatMessage::new("user", prompt_text)];

        let mut request = ChatRequest::new(model, messages);
        request.max_tokens = max_tokens.or(Some(4096));
        request.temperature = temperature;
        request.system = system;
        request.reasoning_effort = reasoning_effort;
        request.timeout_ms = opt_timeout_ms(args.get(1));

        // Inside a scheduler task: offload + yield so siblings overlap. The poller
        // accounts (no post-call `track_usage` here) and shapes the value. The sync
        // branch below is byte-identical to before.
        #[cfg(not(target_arch = "wasm32"))]
        if sema_core::in_async_context() {
            return do_complete_async_yield(
                request,
                Box::new(|resp| Ok(Value::string(&resp.content))),
            );
        }

        let response = do_complete(request)?;
        track_usage(&response.usage)?;
        Ok(Value::string(&response.content))
    });

    // (llm/chat messages {:model "..." :tools [...] :tool-mode :auto ...})
    register_fn_ctx_gated(
        env,
        sandbox,
        sema_core::Caps::LLM,
        "llm/chat",
        |ctx, args| {
            if args.is_empty() || args.len() > 2 {
                return Err(SemaError::arity("llm/chat", "1-2", args.len()));
            }

            let messages = extract_messages(&args[0])?;

            let mut model = String::new();
            let mut max_tokens = None;
            let mut temperature = None;
            let mut system = None;
            let mut reasoning_effort = None;
            let mut tools: Vec<Value> = Vec::new();
            let mut tool_mode = "auto".to_string();
            let mut max_tool_rounds = 10usize;
            let mut on_tool_call: Option<Value> = None;
            let mut conv_scope = ConvScope::default();

            if let Some(opts_val) = args.get(1) {
                if let Some(opts) = opts_val.as_map_rc() {
                    conv_scope = ConvScope::from_opts(Some(&opts));
                    model = get_opt_string(&opts, "model").unwrap_or_default();
                    max_tokens = get_opt_u32(&opts, "max-tokens");
                    temperature = get_opt_f64(&opts, "temperature");
                    system = get_opt_string(&opts, "system");
                    reasoning_effort = get_opt_effort(&opts, "reasoning-effort");
                    // A per-tool-call callback (the workflow `agent` macro passes one to
                    // journal each genuine tool call as an `agent.tool_call` event).
                    on_tool_call = opts.get(&Value::keyword("on-tool-call")).cloned();
                    if let Some(t) = opts.get(&Value::keyword("tools")).and_then(|v| v.as_seq()) {
                        tools = t.to_vec();
                    }
                    if let Some(mode) = opts.get(&Value::keyword("tool-mode")) {
                        if let Some(s) = mode.as_keyword() {
                            tool_mode = s;
                        }
                    }
                    if let Some(rounds) = opts.get(&Value::keyword("max-tool-rounds")) {
                        if let Some(n) = rounds.as_int() {
                            max_tool_rounds = n as usize;
                        }
                    }
                }
            }

            // Per-call observability tags/metadata for both the no-tools (do_complete)
            // and tool-loop (run_tool_loop) branches below. Bound here so the guard
            // outlives the dispatch.
            let _tele = install_call_telemetry(args.get(1).and_then(|v| v.as_map_rc()).as_ref());

            if tools.is_empty() || tool_mode == "none" {
                // Simple chat without tools
                let mut request = ChatRequest::new(model, messages);
                request.max_tokens = max_tokens.or(Some(4096));
                request.temperature = temperature;
                request.system = system;
                request.reasoning_effort = reasoning_effort;
                request.timeout_ms = opt_timeout_ms(args.get(1));
                let _conv = conv_scope.open();
                let response = do_complete(request)?;
                track_usage(&response.usage)?;
                Ok(Value::string(&response.content))
            } else {
                // Chat with tool execution loop
                let tool_schemas = build_tool_schemas(&tools)?;
                let (result, _msgs) = run_tool_loop(
                    ctx,
                    messages,
                    model,
                    max_tokens,
                    temperature,
                    system,
                    reasoning_effort,
                    &tools,
                    &tool_schemas,
                    max_tool_rounds,
                    on_tool_call.as_ref(),
                    None, // on_text: llm/chat doesn't stream
                    None, // agent_name
                    conv_scope,
                )?;
                Ok(Value::string(&result))
            }
        },
    );

    // (llm/send prompt {:model "..." ...})
    register_fn_gated(env, sandbox, sema_core::Caps::LLM, "llm/send", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/send", "1-2", args.len()));
        }
        let prompt = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        complete_with_prompt(&prompt, args.get(1))
    });

    // (llm/stream "prompt" callback {:max-tokens 200})
    // (llm/stream "prompt" {:max-tokens 200})  — prints to stdout
    //
    // The synchronous stream native. The public `llm/stream` is a prelude wrapper
    // that dispatches here outside a scheduler task and to the non-blocking
    // `__stream-begin`/`__stream-next` machinery inside one (siblings interleave
    // between delta batches there).
    register_fn_ctx(env, "__llm-stream-blocking", |ctx, args| {
        let (request, callback, opts_map) = parse_stream_args(args)?;
        let conv_scope = ConvScope::from_opts(opts_map.as_ref());

        // Streaming bypasses do_complete/track_usage, so it gets its own CLIENT span +
        // conversation scope. A caller-supplied id wins; else generate a fresh one (only
        // if no scope is already active).
        let _conv = conv_scope.open().or_else(|| {
            (sema_otel::current_conversation_id().is_none()).then(|| {
                sema_otel::set_conversation_scope(&sema_otel::new_conversation_id(), None, None)
            })
        });
        let span = sema_otel::llm_span("chat");
        span.set_request(
            request.temperature,
            request.max_tokens,
            &request.stop_sequences,
            None,
        );
        span.set_output_type(false);
        // Per-call observability tags/metadata (streaming bypasses do_complete).
        if let Some(ref m) = opts_map {
            let tags = get_opt_string_list(m, "tags");
            if !tags.is_empty() {
                span.set_tags(&tags);
            }
            let meta = get_opt_str_map(m, "metadata");
            if !meta.is_empty() {
                span.set_metadata(&meta);
            }
        }

        // Deliver each chunk to the user callback (or stdout). One callback for both the
        // model-pinned and default-model paths; the dispatch helper resolves the model.
        let mut chunk_cb = |chunk: &str| -> Result<(), crate::types::LlmError> {
            if let Some(ref cb) = callback {
                sema_core::call_callback(ctx, cb, &[Value::string(chunk)])
                    .map_err(|e| crate::types::LlmError::Config(e.to_string()))?;
            } else {
                print!("{}", chunk);
                use std::io::Write;
                std::io::stdout().flush().ok();
            }
            Ok(())
        };
        // Stream-open dispatch: budget pre-gate + rate-limit + fallback-at-open.
        let response = stream_with_dispatch(request, &mut chunk_cb, &span)?;

        // Print newline after streaming if using default display
        if callback.is_none() {
            println!();
        }

        track_usage(&response.usage)?;
        Ok(Value::string(&response.content))
    });

    // (llm/extract schema text {:model "..." :validate true :retries 2 :reask? true})
    register_fn(env, "llm/extract", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/extract", "2-3", args.len()));
        }
        let schema = args[0].clone();
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

        let schema_desc = format_schema(&schema);
        let system = format!(
            "Extract structured data from the text. Respond with ONLY a JSON object matching this schema:\n{}\nDo not include any other text.",
            schema_desc
        );
        let messages = vec![ChatMessage::new("user", text)];

        let mut model = String::new();
        let mut validate = true;
        let mut max_retries: u32 = 2;
        let mut reask = true;
        if let Some(opts_val) = args.get(2) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = get_opt_string(&opts, "model").unwrap_or_default();
                if let Some(v) = opts.get(&Value::keyword("validate")) {
                    validate = v.is_truthy();
                }
                if let Some(r) = get_opt_u32(&opts, "retries") {
                    max_retries = r;
                }
                if let Some(v) = opts.get(&Value::keyword("reask?")) {
                    reask = v.is_truthy();
                }
            }
        }

        // Attempt 0: the initial extraction request.
        let mut request = ChatRequest::new(model.clone(), messages.clone());
        request.json_mode = true;
        request.system = Some(system.clone());

        // Inside a scheduler task: ONLY attempt 0 is offloaded so siblings overlap;
        // the poller accounts attempt 0, then `finalize` validates and — only if a
        // re-ask is needed — runs the remaining attempts on the SYNCHRONOUS
        // `do_complete` path (VM thread). A re-asking extract therefore briefly
        // blocks siblings; the common single-attempt extract is fully concurrent.
        #[cfg(not(target_arch = "wasm32"))]
        if sema_core::in_async_context() {
            let cfg = ExtractConfig {
                schema,
                schema_desc,
                system,
                model,
                messages,
                validate,
                max_retries,
                reask,
            };
            return do_complete_async_yield(
                request,
                Box::new(move |first| extract_validate_and_reask(first, &cfg)),
            );
        }

        // Sync path (byte-identical to before): attempt 0 through `do_complete` +
        // `track_usage`, then the shared validate/re-ask loop.
        let first = do_complete(request)?;
        track_usage(&first.usage)?;
        let cfg = ExtractConfig {
            schema,
            schema_desc,
            system,
            model,
            messages,
            validate,
            max_retries,
            reask,
        };
        extract_validate_and_reask(first, &cfg)
    });

    // (llm/extract-from-image schema source {:model "..."})
    // source: string path or bytevector
    register_fn_ctx_gated(
        env,
        sandbox,
        sema_core::Caps::LLM,
        "llm/extract-from-image",
        |_ctx, args| {
            if args.len() < 2 || args.len() > 3 {
                return Err(SemaError::arity(
                    "llm/extract-from-image",
                    "2-3",
                    args.len(),
                ));
            }
            let schema = &args[0];

            // Get image bytes: either from path (string) or bytevector
            let bytes = if let Some(path) = args[1].as_str() {
                std::fs::read(path)
                    .map_err(|e| SemaError::Io(format!("llm/extract-from-image: {path}: {e}")))?
            } else if let Some(bv) = args[1].as_bytevector() {
                bv.to_vec()
            } else {
                return Err(SemaError::type_error(
                    "string path or bytevector",
                    args[1].type_name(),
                ));
            };

            let media_type = detect_media_type(&bytes).to_string();
            use base64::Engine;
            let b64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

            let schema_desc = format_schema(schema);
            let system = format!(
                "Extract structured data from the image. Respond with ONLY a JSON object matching this schema:\n{}\nDo not include any other text.",
                schema_desc
            );

            let messages = vec![ChatMessage::with_blocks(
                "user",
                vec![
                    ContentBlock::Image {
                        media_type: Some(media_type),
                        data: b64_data,
                    },
                    ContentBlock::Text {
                        text: "Extract the requested data from this image. Respond in JSON."
                            .to_string(),
                    },
                ],
            )];

            let mut model = String::new();
            if let Some(opts_val) = args.get(2) {
                if let Some(opts) = opts_val.as_map_rc() {
                    model = get_opt_string(&opts, "model").unwrap_or_default();
                }
            }

            let mut request = ChatRequest::new(model, messages);
            request.system = Some(system);
            request.json_mode = true;

            let response = do_complete(request)?;
            track_usage(&response.usage)?;

            // Parse JSON response back to Sema value
            let content = response.content.trim();
            let json_str = if content.starts_with("```") {
                content
                    .trim_start_matches("```json")
                    .trim_start_matches("```")
                    .trim_end_matches("```")
                    .trim()
            } else {
                content
            };
            let json: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
                SemaError::Llm(format!(
                    "failed to parse LLM JSON response: {e}\nResponse was: {content}"
                ))
            })?;
            Ok(sema_core::json_to_value(&json))
        },
    );

    // (llm/classify categories text {:model "..."})
    register_fn(env, "llm/classify", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/classify", "2-3", args.len()));
        }
        let categories = args[0]
            .as_seq()
            .map(|l| l.to_vec())
            .ok_or_else(|| SemaError::type_error("list or vector", args[0].type_name()))?;
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

        let cat_names: Vec<String> = categories
            .iter()
            .map(|c| {
                if let Some(kw) = c.as_keyword() {
                    kw
                } else if let Some(s) = c.as_str() {
                    s.to_string()
                } else {
                    c.to_string()
                }
            })
            .collect();

        let system = format!(
            "Classify the following text into exactly one of these categories: {}\nRespond with ONLY the category name, nothing else.",
            cat_names.join(", ")
        );
        let messages = vec![ChatMessage::new("user", text)];

        let mut model = String::new();
        if let Some(opts_val) = args.get(2) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = get_opt_string(&opts, "model").unwrap_or_default();
            }
        }

        let mut request = ChatRequest::new(model, messages);
        request.system = Some(system);

        // Shape the response into a category keyword (if it matched a keyword in the
        // original list) or string. Shared by the sync and async paths.
        let parse_category = move |response: ChatResponse| -> Result<Value, SemaError> {
            let category = response.content.trim().to_string();
            if categories
                .iter()
                .any(|c| c.as_keyword().map(|kw| kw == category).unwrap_or(false))
            {
                Ok(Value::keyword(&category))
            } else {
                Ok(Value::string(&category))
            }
        };

        // Inside a scheduler task: offload + yield so siblings overlap; the poller
        // accounts and runs `parse_category`. Sync branch is byte-identical.
        #[cfg(not(target_arch = "wasm32"))]
        if sema_core::in_async_context() {
            return do_complete_async_yield(request, Box::new(parse_category));
        }

        let response = do_complete(request)?;
        track_usage(&response.usage)?;
        parse_category(response)
    });

    // Conversation functions

    // (conversation/new {:model "..."})
    register_fn(env, "conversation/new", |args| {
        let mut model = String::new();
        let mut metadata = BTreeMap::new();
        if let Some(opts_val) = args.first() {
            if let Some(opts) = opts_val.as_map_rc() {
                model = get_opt_string(&opts, "model").unwrap_or_default();
                for (k, v) in opts.iter() {
                    if let Some(key_str) = k.as_keyword() {
                        if key_str != "model" {
                            metadata.insert(
                                key_str,
                                v.as_str()
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| v.to_string()),
                            );
                        }
                    }
                }
            }
        }
        Ok(Value::conversation(Conversation {
            messages: Vec::new(),
            model,
            metadata,
        }))
    });

    // (conversation/say conv "message" {:temperature 0.5 :max-tokens 2048 :system "..."})
    register_fn(env, "conversation/say", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("conversation/say", "2-3", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let user_msg = args[1]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[1].to_string());

        // Parse optional opts
        let mut temperature = None;
        let mut max_tokens = None;
        let mut system = None;
        if let Some(opts_val) = args.get(2) {
            if let Some(opts) = opts_val.as_map_rc() {
                temperature = get_opt_f64(&opts, "temperature");
                max_tokens = get_opt_u32(&opts, "max-tokens");
                system = get_opt_string(&opts, "system");
            }
        }

        // Build messages for API call
        let mut chat_messages: Vec<ChatMessage> = conv
            .messages
            .iter()
            .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
            .collect();
        chat_messages.push(ChatMessage::new("user", user_msg.clone()));

        let mut request = ChatRequest::new(conv.model.clone(), chat_messages);
        request.temperature = temperature;
        request.max_tokens = max_tokens.or(Some(4096));
        request.system = system;

        let response = do_complete(request)?;
        track_usage(&response.usage)?;

        // Build new conversation with user message + assistant reply
        let mut new_messages = conv.messages.clone();
        new_messages.push(Message {
            role: Role::User,
            content: user_msg,
            images: Vec::new(),
        });
        new_messages.push(Message {
            role: Role::Assistant,
            content: response.content,
            images: Vec::new(),
        });

        let mut metadata = conv.metadata.clone();
        accumulate_usage(&mut metadata, &response.usage);
        Ok(Value::conversation(Conversation {
            messages: new_messages,
            model: conv.model.clone(),
            metadata,
        }))
    });

    // (conversation/messages conv)
    register_fn(env, "conversation/messages", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/messages", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let msgs: Vec<Value> = conv
            .messages
            .iter()
            .map(|m| Value::message(m.clone()))
            .collect();
        Ok(Value::list(msgs))
    });

    // (conversation/last-reply conv)
    register_fn(env, "conversation/last-reply", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/last-reply", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        conv.messages
            .iter()
            .rfind(|m| m.role == Role::Assistant)
            .map(|m| Value::string(&m.content))
            .ok_or_else(|| SemaError::eval("no assistant reply in conversation"))
    });

    // (conversation/fork conv)
    register_fn(env, "conversation/fork", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/fork", "1", args.len()));
        }
        // Fork returns a copy - since conversations are immutable, this is just clone
        Ok(args[0].clone())
    });

    // Prompt functions

    // (prompt/append p1 p2 ...) — variadic, concatenates all prompts
    register_fn(env, "prompt/append", |args| {
        if args.is_empty() {
            return Err(SemaError::arity("prompt/append", "1+", args.len()));
        }
        let mut messages = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            let p = arg
                .as_prompt_rc()
                .ok_or_else(|| SemaError::type_error("prompt", args[i].type_name()))?;
            messages.extend(p.messages.iter().cloned());
        }
        Ok(Value::prompt(Prompt { messages }))
    });

    // (prompt/concat p1 p2 ...) — alias for variadic prompt/append
    register_fn(env, "prompt/concat", |args| {
        if args.is_empty() {
            return Err(SemaError::arity("prompt/concat", "1+", args.len()));
        }
        let mut messages = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            let p = arg
                .as_prompt_rc()
                .ok_or_else(|| SemaError::type_error("prompt", args[i].type_name()))?;
            messages.extend(p.messages.iter().cloned());
        }
        Ok(Value::prompt(Prompt { messages }))
    });

    // (prompt/messages prompt)
    register_fn(env, "prompt/messages", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("prompt/messages", "1", args.len()));
        }
        let p = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let msgs: Vec<Value> = p
            .messages
            .iter()
            .map(|m| Value::message(m.clone()))
            .collect();
        Ok(Value::list(msgs))
    });

    // (prompt/set-system prompt "new system message")
    register_fn(env, "prompt/set-system", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/set-system", "2", args.len()));
        }
        let p = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let new_system = args[1]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[1].to_string());
        let mut messages: Vec<Message> = p
            .messages
            .iter()
            .filter(|m| m.role != Role::System)
            .cloned()
            .collect();
        messages.insert(
            0,
            Message {
                role: Role::System,
                content: new_system,
                images: Vec::new(),
            },
        );
        Ok(Value::prompt(Prompt { messages }))
    });

    // (message/role msg)
    register_fn(env, "message/role", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("message/role", "1", args.len()));
        }
        let msg = args[0]
            .as_message_rc()
            .ok_or_else(|| SemaError::type_error("message", args[0].type_name()))?;
        Ok(Value::keyword(&msg.role.to_string()))
    });

    // (message/content msg)
    register_fn(env, "message/content", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("message/content", "1", args.len()));
        }
        let msg = args[0]
            .as_message_rc()
            .ok_or_else(|| SemaError::type_error("message", args[0].type_name()))?;
        Ok(Value::string(&msg.content))
    });

    // Usage tracking

    // (llm/last-usage)
    register_fn(env, "llm/last-usage", |_args| {
        LAST_USAGE.with(|u| {
            let u = u.borrow();
            match &*u {
                Some(usage) => {
                    let mut map = BTreeMap::new();
                    map.insert(
                        Value::keyword("prompt-tokens"),
                        Value::int(usage.prompt_tokens as i64),
                    );
                    map.insert(
                        Value::keyword("completion-tokens"),
                        Value::int(usage.completion_tokens as i64),
                    );
                    map.insert(
                        Value::keyword("total-tokens"),
                        Value::int(usage.total_tokens() as i64),
                    );
                    map.insert(
                        Value::keyword("cache-read-tokens"),
                        Value::int(usage.cache_read_input_tokens as i64),
                    );
                    map.insert(
                        Value::keyword("cache-creation-tokens"),
                        Value::int(usage.cache_creation_input_tokens as i64),
                    );
                    map.insert(Value::keyword("model"), Value::string(&usage.model));
                    if let Some(cost) = pricing::calculate_cost(usage) {
                        map.insert(Value::keyword("cost-usd"), Value::float(cost));
                    }
                    Ok(Value::map(map))
                }
                None => Ok(Value::nil()),
            }
        })
    });

    // (llm/session-usage)
    register_fn(env, "llm/session-usage", |_args| {
        SESSION_USAGE.with(|u| {
            let usage = u.borrow();
            let mut map = BTreeMap::new();
            map.insert(
                Value::keyword("prompt-tokens"),
                Value::int(usage.prompt_tokens as i64),
            );
            map.insert(
                Value::keyword("completion-tokens"),
                Value::int(usage.completion_tokens as i64),
            );
            map.insert(
                Value::keyword("total-tokens"),
                Value::int(usage.total_tokens() as i64),
            );
            map.insert(
                Value::keyword("cache-read-tokens"),
                Value::int(usage.cache_read_input_tokens as i64),
            );
            map.insert(
                Value::keyword("cache-creation-tokens"),
                Value::int(usage.cache_creation_input_tokens as i64),
            );
            let session_cost = SESSION_COST.with(|sc| *sc.borrow());
            map.insert(Value::keyword("cost-usd"), Value::float(session_cost));
            Ok(Value::map(map))
        })
    });

    // (agent/run agent "msg") returns string
    // (agent/run agent "msg" {:on-tool-call cb :messages history}) returns {:response "..." :messages [...]}
    // Synchronous / wasm agent loop (byte-identical to the historical `agent/run`).
    // The `agent/run` name is bound in the prelude to a dispatcher that reaches this
    // native in non-async context and the yield-per-round driver in async context.
    register_fn_ctx(env, "__agent-run-blocking", |ctx, args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("agent/run", "2-3", args.len()));
        }
        let agent = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        let user_msg = args[1]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[1].to_string());

        // Extract options from 3rd arg
        let opts = args.get(2).and_then(|v| v.as_map_rc());

        let on_tool_call = opts
            .as_ref()
            .and_then(|o| o.get(&Value::keyword("on-tool-call")).cloned());

        // Optional streaming hook: called with each assistant text delta so a TUI
        // can render the reply live. Absent → non-streaming (unchanged) behavior.
        let on_text = opts
            .as_ref()
            .and_then(|o| o.get(&Value::keyword("on-text")).cloned());

        // Optional per-run reasoning effort, e.g. (agent/run a msg {:reasoning-effort :high}).
        let reasoning_effort = opts
            .as_ref()
            .and_then(|o| get_opt_effort(o, "reasoning-effort"));

        // ── Phase 1: :session input — seed history from a prior Conversation ──
        // When :session is a Conversation value, extract its messages as the initial
        // history (so turn 2 sees turn 1's full history). Also extract its
        // conversation-id so telemetry threads across turns.
        let (session_messages, session_conv_id): (Vec<ChatMessage>, Option<String>) =
            if let Some(ref o) = opts {
                if let Some(sess_val) = o.get(&Value::keyword("session")) {
                    if let Some(conv_rc) = sess_val.as_conversation_rc() {
                        let msgs: Vec<ChatMessage> = conv_rc
                            .messages
                            .iter()
                            .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
                            .collect();
                        let cid = conv_rc.metadata.get("conversation-id").cloned();
                        (msgs, cid)
                    } else {
                        (Vec::new(), None)
                    }
                } else {
                    (Vec::new(), None)
                }
            } else {
                (Vec::new(), None)
            };

        // ── :memory opt — seed from memory working set ────────────────────────
        // If :memory is given and memory callbacks are registered, extract the working
        // messages and prepend them before any :session messages. After the run, the
        // new turns are appended back into the memory.
        let memory_handle: Option<Value> = opts
            .as_ref()
            .and_then(|o| o.get(&Value::keyword("memory")).cloned());

        let memory_seed: Vec<ChatMessage> = if let Some(ref h) = memory_handle {
            MEMORY_CALLBACKS.with(|c| {
                if let Some(ref cbs) = *c.borrow() {
                    (cbs.get_working)(h).unwrap_or_default()
                } else {
                    Vec::new()
                }
            })
        } else {
            Vec::new()
        };
        let memory_seed_len = memory_seed.len();

        // Generate (or reuse) the conversation-id BEFORE run_tool_loop so we can
        // attach it to the :session output conversation. Explicit :conversation-id opt
        // wins; then the :session's stored id; otherwise generate a fresh one.
        let output_conv_id: String = session_conv_id
            .clone()
            .or_else(|| {
                opts.as_ref()
                    .and_then(|o| o.get(&Value::keyword("conversation-id")))
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_else(sema_otel::new_conversation_id);

        let conv_scope = ConvScope {
            conversation: Some(output_conv_id.clone()),
            session: opts
                .as_ref()
                .and_then(|o| o.get(&Value::keyword("session-id")))
                .and_then(|v| v.as_str().map(|s| s.to_string())),
            user: opts
                .as_ref()
                .and_then(|o| o.get(&Value::keyword("user-id")))
                .and_then(|v| v.as_str().map(|s| s.to_string())),
        };

        // Build messages: memory working set + session history + :messages history + new user.
        // Track the pre-user-push count so we can slice new turns for memory append.
        let mut messages: Vec<ChatMessage> = memory_seed;
        messages.extend(session_messages);
        if let Some(ref o) = opts {
            if let Some(history) = o.get(&Value::keyword("messages")) {
                let extra = sema_list_to_chat_messages(history)?;
                messages.extend(extra);
            }
        }
        // Capture the index of the first NEW turn (user+assistant) before the user push.
        let pre_user_count = messages.len();
        messages.push(ChatMessage::new("user", user_msg));

        let tool_schemas = build_tool_schemas(&agent.tools)?;
        let system = if agent.system.is_empty() {
            None
        } else {
            Some(agent.system.clone())
        };

        // Per-run observability tags/metadata: attached to the agent span (and inherited
        // by the nested per-round chat spans) inside run_tool_loop.
        let _tele = install_call_telemetry(opts.as_ref());

        let (result, final_messages) = run_tool_loop(
            ctx,
            messages,
            agent.model.clone(),
            Some(4096),
            None,
            system,
            reasoning_effort,
            &agent.tools,
            &tool_schemas,
            agent.max_turns,
            on_tool_call.as_ref(),
            on_text.as_ref(),
            Some(&agent.name),
            conv_scope,
        )?;

        // ── :memory post-run: append new turns back into the memory thread ────
        // Append turns from pre_user_count onward (user turn + new assistant turns).
        // This excludes the memory seed (already in memory) but includes session/extra
        // history only if it was new (which is correct — those are the new turns).
        // We want to persist user + assistant: slice from pre_user_count.
        if let Some(ref h) = memory_handle {
            let new_turns = if final_messages.len() > pre_user_count {
                &final_messages[pre_user_count..]
            } else {
                &[]
            };
            let _ = memory_seed_len; // consumed above, silence warning
            MEMORY_CALLBACKS.with(|c| {
                if let Some(ref cbs) = *c.borrow() {
                    let _ = (cbs.append_back)(h, new_turns);
                }
            });
        }

        // 3-arg form with opts: return {:response "..." :messages [...] :session <conv>}
        if opts.is_some() {
            let mut meta = std::collections::BTreeMap::new();
            meta.insert("conversation-id".to_string(), output_conv_id);
            let session_conv = Conversation {
                messages: final_messages
                    .iter()
                    .map(|m| Message {
                        role: match m.role.as_str() {
                            "assistant" => Role::Assistant,
                            _ => Role::User,
                        },
                        content: m.content.to_text(),
                        images: Vec::new(),
                    })
                    .collect(),
                model: agent.model.clone(),
                metadata: meta,
            };
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("response"), Value::string(&result));
            map.insert(
                Value::keyword("messages"),
                chat_messages_to_sema_list(&final_messages),
            );
            map.insert(Value::keyword("session"), Value::conversation(session_conv));
            Ok(Value::map(map))
        } else {
            // 2-arg form: return string (backward compat)
            Ok(Value::string(&result))
        }
    });

    // ── Non-blocking multi-round agent loop (async-context path) ──────────────
    // The prelude `agent/run` dispatches here (four internal natives + a Sema
    // driver loop) when `(__async-context?)`, so each provider round offloads +
    // yields `AwaitIo` and sibling scheduler tasks overlap during the conversation.
    // See docs/plans/2026-07-02-nonblocking-agent-run.md (ADR #68).
    register_fn_ctx(env, "__async-context?", |_ctx, _args| {
        Ok(Value::bool(sema_core::in_async_context()))
    });
    register_fn_ctx(env, "__agent-begin", |_ctx, args| agent_begin(args));
    register_fn_ctx(env, "__agent-step", |ctx, args| {
        let token = agent_token_arg(args, "__agent-step")?;
        agent_step(ctx, token)
    });
    register_fn_ctx(env, "__agent-exec-tools", |ctx, args| {
        let token = agent_token_arg(args, "__agent-exec-tools")?;
        agent_exec_tools(ctx, token)
    });
    register_fn_ctx(env, "__agent-finish", |_ctx, args| {
        let token = agent_token_arg(args, "__agent-finish")?;
        agent_finish(token)
    });

    // Non-blocking streaming natives (the `__stream-drive` prelude loop's
    // primitives; same bytecode-driven shape as the `__agent-*` loop above).
    register_fn(env, "__stream-begin", |args| {
        let (request, _callback, opts_map) = parse_stream_args(args)?;
        let conv_scope = ConvScope::from_opts(opts_map.as_ref());
        // Same scope rule as the blocking native: a caller-supplied id wins;
        // else generate a fresh one only if no scope is already active. The
        // DETACHED span captures the conversation id at creation, so the guard
        // need only live across span creation.
        let _conv = conv_scope.open().or_else(|| {
            (sema_otel::current_conversation_id().is_none()).then(|| {
                sema_otel::set_conversation_scope(&sema_otel::new_conversation_id(), None, None)
            })
        });
        let span = sema_otel::llm_span_detached("chat");
        span.set_request(
            request.temperature,
            request.max_tokens,
            &request.stop_sequences,
            None,
        );
        span.set_output_type(false);
        if let Some(ref m) = opts_map {
            let tags = get_opt_string_list(m, "tags");
            if !tags.is_empty() {
                span.set_tags(&tags);
            }
            let meta = get_opt_str_map(m, "metadata");
            if !meta.is_empty() {
                span.set_metadata(&meta);
            }
        }
        stream_run_begin(request, span)
    });
    register_fn(env, "__stream-next", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("__stream-next", "1", args.len()));
        }
        stream_next(stream_token_arg(&args[0])?)
    });
    register_fn(env, "__stream-finish", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("__stream-finish", "1", args.len()));
        }
        stream_finish(stream_token_arg(&args[0])?)
    });
    register_fn(env, "__agent-stream-apply", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("__agent-stream-apply", "2", args.len()));
        }
        let agent_token = agent_token_arg(&args[..1], "__agent-stream-apply")?;
        agent_stream_apply(agent_token, stream_token_arg(&args[1])?)
    });

    // (llm/pmap fn collection {:max-tokens N ...})
    // Maps fn over collection to produce prompts, then sends all prompts in parallel via batch_complete
    register_fn_ctx(env, "llm/pmap", |ctx, args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/pmap", "2-3", args.len()));
        }
        let func = &args[0];
        let items = args[1]
            .as_seq()
            .map(|l| l.to_vec())
            .ok_or_else(|| SemaError::type_error("list or vector", args[1].type_name()))?;

        let mut model = String::new();
        let mut max_tokens = None;
        let mut temperature = None;
        let mut system = None;

        if let Some(opts_val) = args.get(2) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = get_opt_string(&opts, "model").unwrap_or_default();
                max_tokens = get_opt_u32(&opts, "max-tokens");
                temperature = get_opt_f64(&opts, "temperature");
                system = get_opt_string(&opts, "system");
            }
        }

        // Step 1: Map fn over items to produce prompt strings (sequentially, since Rc)
        let mut prompts = Vec::with_capacity(items.len());
        for item in &items {
            #[allow(clippy::cloned_ref_to_slice_refs)] // clone needed: &Value -> [Value]
            let result = sema_core::call_callback(ctx, func, &[item.clone()])?;
            let prompt_str = result
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| result.to_string());
            prompts.push(prompt_str);
        }

        // Step 2: Build ChatRequests
        let requests: Vec<ChatRequest> = prompts
            .into_iter()
            .map(|prompt_text| {
                let messages = vec![ChatMessage::new("user", prompt_text)];
                let mut req = ChatRequest::new(model.clone(), messages);
                req.max_tokens = max_tokens.or(Some(4096));
                req.temperature = temperature;
                req.system = system.clone();
                req
            })
            .collect();

        // Step 3: batch_complete (runs concurrently at provider level)
        let responses = with_provider(|p| {
            let reqs: Vec<ChatRequest> = requests
                .into_iter()
                .map(|mut r| {
                    if r.model.is_empty() {
                        r.model = p.default_model().to_string();
                    }
                    r
                })
                .collect();
            Ok(p.batch_complete(reqs))
        })?;

        // Step 4: Collect results
        let mut results = Vec::with_capacity(responses.len());
        for resp_result in responses {
            let resp = resp_result.map_err(|e| SemaError::Llm(e.to_string()))?;
            track_usage(&resp.usage)?;
            results.push(Value::string(&resp.content));
        }
        Ok(Value::list(results))
    });

    // (llm/batch ["prompt1" "prompt2" "prompt3"] {:max-tokens 100})
    register_fn(env, "llm/batch", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/batch", "1-2", args.len()));
        }
        let prompts = args[0]
            .as_seq()
            .map(|l| l.to_vec())
            .ok_or_else(|| SemaError::type_error("list or vector", args[0].type_name()))?;

        let mut model = String::new();
        let mut max_tokens = None;
        let mut temperature = None;
        let mut system = None;

        if let Some(opts_val) = args.get(1) {
            if let Some(opts) = opts_val.as_map_rc() {
                model = get_opt_string(&opts, "model").unwrap_or_default();
                max_tokens = get_opt_u32(&opts, "max-tokens");
                temperature = get_opt_f64(&opts, "temperature");
                system = get_opt_string(&opts, "system");
            }
        }

        let requests: Vec<ChatRequest> = prompts
            .iter()
            .map(|prompt_val| {
                let prompt_text = prompt_val
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| prompt_val.to_string());
                let messages = vec![ChatMessage::new("user", prompt_text)];
                let mut req = ChatRequest::new(model.clone(), messages);
                req.max_tokens = max_tokens.or(Some(4096));
                req.temperature = temperature;
                req.system = system.clone();
                req
            })
            .collect();

        let responses = with_provider(|p| {
            let reqs: Vec<ChatRequest> = requests
                .into_iter()
                .map(|mut r| {
                    if r.model.is_empty() {
                        r.model = p.default_model().to_string();
                    }
                    r
                })
                .collect();
            Ok(p.batch_complete(reqs))
        })?;

        let mut results = Vec::with_capacity(responses.len());
        for resp_result in responses {
            let resp = resp_result.map_err(|e| SemaError::Llm(e.to_string()))?;
            track_usage(&resp.usage)?;
            results.push(Value::string(&resp.content));
        }
        Ok(Value::list(results))
    });

    // (llm/set-pricing "model-pattern" input-per-million output-per-million)
    register_fn(env, "llm/set-pricing", |args| {
        if args.len() != 3 {
            return Err(SemaError::arity("llm/set-pricing", "3", args.len()));
        }
        let model_pattern = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let input_cost = args[1]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[1].type_name()))?;
        let output_cost = args[2]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[2].type_name()))?;
        pricing::set_custom_pricing(model_pattern, input_cost, output_cost);
        Ok(Value::nil())
    });

    // (llm/configure-embeddings :openai {:api-key "..." :base-url "..." :model "..."})
    // (llm/configure-embeddings :jina {:api-key "..."})
    // (llm/configure-embeddings :voyage {:api-key "..."})
    // (llm/configure-embeddings :cohere {:api-key "..."})
    register_fn(env, "llm/configure-embeddings", move |args| {
        if args.len() != 2 {
            return Err(SemaError::arity(
                "llm/configure-embeddings",
                "2",
                args.len(),
            ));
        }
        let provider_name = args[0]
            .as_keyword()
            .ok_or_else(|| SemaError::type_error("keyword", args[0].type_name()))?;
        let opts_rc = args[1]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        let opts = opts_rc.as_ref().clone();

        guard_provider_url(unrestricted, &opts)?;

        let api_key = get_opt_string(&opts, "api-key");

        PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            match provider_name.as_str() {
                "jina" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "jina-embeddings-v3".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "jina".to_string(),
                        api_key,
                        "https://api.jina.ai/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("jina");
                }
                "voyage" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model")
                        .unwrap_or_else(|| "voyage-3".to_string());
                    let provider = OpenAiCompatEmbeddingProvider::new(
                        "voyage".to_string(),
                        api_key,
                        "https://api.voyageai.com/v1".to_string(),
                        model,
                    )
                    .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("voyage");
                }
                "cohere" => {
                    let api_key = api_key
                        .clone()
                        .ok_or_else(|| SemaError::Llm("missing :api-key".to_string()))?;
                    let model = get_opt_string(&opts, "default-model");
                    let provider = CohereEmbeddingProvider::new(api_key, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider("cohere");
                }
                _ => {
                    // Default: OpenAI-compatible
                    let api_key = api_key.unwrap_or_default();
                    let base_url = get_opt_string(&opts, "base-url");
                    let model = get_opt_string(&opts, "default-model")
                        .or_else(|| get_opt_string(&opts, "model"));
                    let provider = OpenAiProvider::new(api_key, base_url, model)
                        .map_err(|e| SemaError::Llm(e.to_string()))?;
                    let name = provider.name().to_string();
                    reg.register(Box::new(provider));
                    reg.set_embedding_provider(&name);
                }
            }
            Ok(Value::nil())
        })
    });

    // `llm/embed` — a SINGLE first-class native function that branches internally
    // on `sema_core::in_async_context()`:
    //
    //   (llm/embed "text" {:model "..."})        ; → bytevector
    //   (llm/embed ["text1" "text2"] {:model …}) ; → list of bytevectors
    //
    // Outside an async scheduler task it runs the SYNCHRONOUS embed path inline
    // (open span, cassette, provider.embed, set_response, track_usage, decode).
    // Inside a task it offloads `provider.embed` onto the shared runtime and
    // yields `AwaitIo` so sibling tasks overlap; the IoHandle poller (which runs
    // on the VM thread inside the scheduler) finalizes the DETACHED span, records
    // the cassette, runs `track_usage`, and decodes the embeddings into the SAME
    // Value the sync path returns — so the concurrent and sync paths are
    // byte-identical. Folding `track_usage` into the poller is what lets a single
    // native (which is NOT re-invoked on resume — the scheduler resumes the
    // bytecode after the CALL via `replace_stack_top`) account correctly without
    // a separate Sema-sequenced accounting step.
    //
    // Keeping it a native (not a macro) means `(procedure? llm/embed)` is #t and
    // it is usable as a value: `(map llm/embed …)`, `(async/pool-map llm/embed …)`.
    register_fn(env, "llm/embed", |args| {
        // On resume from the async yield the scheduler re-runs the bytecode AFTER
        // this CALL via `replace_stack_top`, so this native is not re-invoked.
        // Drain any stray resume value defensively (mirrors io-sleep-once).
        if let Some(v) = sema_core::take_resume_value() {
            return Ok(v);
        }
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/embed", "1-2", args.len()));
        }

        let (texts, single) = if let Some(s) = args[0].as_str() {
            (vec![s.to_string()], true)
        } else if let Some(l) = args[0].as_seq() {
            let texts: Vec<String> = l
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| v.to_string())
                })
                .collect();
            (texts, false)
        } else {
            return Err(SemaError::type_error("string or list", args[0].type_name()));
        };

        let model = if let Some(opts_val) = args.get(1) {
            if let Some(opts) = opts_val.as_map_rc() {
                get_opt_string(&opts, "model")
            } else {
                None
            }
        } else {
            None
        };

        let request = EmbedRequest { texts, model };
        let req_model = request.model.clone().unwrap_or_default();
        let cassette_key = compute_embed_key(&request);

        // ── ASYNC path: offload + yield (native targets only) ──────────────
        //
        // The concurrent embed path is native-only (no shared tokio runtime on
        // wasm), so wasm always falls through to the synchronous path below.
        #[cfg(not(target_arch = "wasm32"))]
        if sema_core::in_async_context() {
            // DETACHED embeddings span: parent captured now, finalized in the
            // poller after the yield (where the active-span stack may hold a
            // sibling task's span, so the span must not pop the stack on drop).
            let span = sema_otel::llm_span_detached("embeddings");
            span.set_embedding_input(&request.texts);

            // Cassette decision — SYNCHRONOUSLY, pre-spawn, on the VM thread.
            let decision =
                CASSETTE.with(|c| c.borrow().as_ref().map(|cass| cass.decide(&cassette_key)));
            match decision {
                Some(crate::cassette::Decision::Replay(entry)) => {
                    // Replay made no provider call → finalize the span inline,
                    // account, and return WITHOUT yielding (nothing to overlap).
                    let resp = EmbedResponse {
                        embeddings: entry.embeddings,
                        model: entry.model.clone(),
                        usage: Usage {
                            prompt_tokens: entry.prompt_tokens,
                            model: entry.model,
                            ..Default::default()
                        },
                    };
                    span.set_dispatch("cassette", &req_model);
                    span.set_response(&sema_otel::ResponseFacts {
                        input_tokens: resp.usage.prompt_tokens,
                        output_tokens: 0,
                        response_model: resp.model.clone(),
                        ..Default::default()
                    });
                    drop(span);
                    track_usage(&resp.usage)?;
                    return Ok(embed_value_from_response(&resp, single));
                }
                Some(crate::cassette::Decision::Miss(k)) => return Err(cassette_miss_error(&k)),
                _ => {}
            }
            let recording = matches!(decision, Some(crate::cassette::Decision::Record));

            // Clone an Arc<provider> off the thread-local registry on THIS thread,
            // release the borrow, and move it into the offloaded future.
            let provider = PROVIDER_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                reg.embedding_provider().or_else(|| reg.default_provider())
            });
            let Some(provider) = provider else {
                return Err(SemaError::Llm(
                    "no embedding provider configured. Use (llm/configure-embeddings ...) first"
                        .to_string(),
                ));
            };

            // The provider name + canonical price are needed on the VM thread in
            // the poller; capture them before the Arc is moved into the worker.
            let provider_name = provider.name().to_string();
            // Capture the dispatch-time budget + leaf-usage frames (ASYNC-1), so the
            // poller charges the frames active NOW — not whatever scope is installed
            // when the future lands. Mirrors do_complete_async_yield.
            let usage_accum_slot = current_usage_accum();
            let budget_slot = active_budget();
            let (tx, mut rx) = tokio::sync::oneshot::channel::<Result<EmbedResponse, LlmError>>();
            let req2 = request.clone();
            // Spawned pool future (the http/shell abort tier): providers with a
            // native async embed path (`embed_future`) are dropped mid-flight on
            // abort — true cancellation; sync-only providers fall back to the
            // admission-controlled blocking tier, where cancel stays best-effort
            // (result discarded, call runs to completion).
            let abort = sema_io::io_spawn(async move {
                let r = match provider.embed_future(req2.clone()) {
                    Some(fut) => fut.await,
                    None => {
                        let p = provider.clone();
                        sema_io::io_offload_blocking(move || p.embed(req2)).await
                    }
                };
                let _ = tx.send(r);
                sema_core::notify_io_complete();
            });

            // Move the LlmSpan + cassette context INTO the poller closure so the
            // span is finalized (and the cassette recorded + usage accounted) on
            // the VM thread when the future lands — never as a native-frame local
            // (those drop at the yield).
            let key = cassette_key;
            let mut span_slot = Some(span);
            // On cancel/timeout the scheduler runs the abort hook, aborting the
            // spawned wire future. Never called on normal completion.
            let handle = Rc::new(sema_core::IoHandle::with_abort(
                move || {
                    use tokio::sync::oneshot::error::TryRecvError;
                    match rx.try_recv() {
                        Err(TryRecvError::Empty) => sema_core::IoPoll::Pending,
                        Ok(Ok(resp)) => {
                            if let Some(span) = span_slot.take() {
                                span.set_dispatch(&provider_name, &req_model);
                                span.set_response(&sema_otel::ResponseFacts {
                                    input_tokens: resp.usage.prompt_tokens,
                                    output_tokens: 0,
                                    response_model: resp.model.clone(),
                                    cost_usd: pricing::calculate_cost_for(
                                        &provider_name,
                                        &resp.usage,
                                    ),
                                    ..Default::default()
                                });
                                // span drops here → ends the span.
                            }
                            if recording {
                                CASSETTE.with(|c| {
                                    if let Some(cass) = c.borrow_mut().as_mut() {
                                        cass.record_entry(crate::cassette::TapeEntry::from_embed(
                                            &key,
                                            &resp.model,
                                            &resp.embeddings,
                                            resp.usage.prompt_tokens,
                                        ));
                                    }
                                });
                            }
                            // Decode the embedding (byte-identical to the sync path)
                            // and account on the VM thread. `track_usage` only mutates
                            // session-usage / budget thread-locals and reads static
                            // pricing — it never spawns, yields, or touches the
                            // scheduler — so it is safe to call here inside
                            // `wake_blocked_tasks`'s `&mut self.tasks` borrow. Fold into
                            // the CAPTURED leaf-usage frame and charge the CAPTURED
                            // budget frame (ASYNC-1, mirroring the completion poller):
                            // the poller runs outside the per-task install boundary, so
                            // the live thread-locals may belong to a sibling. A budget
                            // overrun fails the task, exactly as the sync path's `?`.
                            if let Some(slot) = &usage_accum_slot {
                                let cost = pricing::calculate_cost_for(&provider_name, &resp.usage);
                                accumulate_into(slot, &resp.usage, cost);
                            }
                            let value = embed_value_from_response(&resp, single);
                            let track_result = {
                                let prev_budget = ACTIVE_BUDGET.with(|b| {
                                    std::mem::replace(&mut *b.borrow_mut(), budget_slot.clone())
                                });
                                let r = USAGE_ACCUM_SUPPRESS.with(|s| {
                                    s.set(true);
                                    let r = track_usage(&resp.usage);
                                    s.set(false);
                                    r
                                });
                                ACTIVE_BUDGET.with(|b| *b.borrow_mut() = prev_budget);
                                r
                            };
                            match track_result {
                                Ok(()) => sema_core::IoPoll::Ready(Ok(value)),
                                Err(e) => sema_core::IoPoll::Ready(Err(e.to_string())),
                            }
                        }
                        Ok(Err(e)) => {
                            if let Some(span) = span_slot.take() {
                                span.record_error(llm_error_kind(&e), &e.to_string());
                            }
                            sema_core::IoPoll::Ready(Err(e.to_string()))
                        }
                        Err(TryRecvError::Closed) => {
                            span_slot.take();
                            sema_core::IoPoll::Ready(Err("embed: io worker dropped".to_string()))
                        }
                    }
                },
                abort,
            ));
            sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
            return Ok(Value::nil());
        }

        // ── SYNC path: inline provider call (byte-identical to before) ─────
        // CLIENT embeddings span (bypasses do_complete). Input tokens only.
        let span = sema_otel::llm_span("embeddings");
        // Advertise the input texts (content-gated; OpenInference embedding.* keys).
        span.set_embedding_input(&request.texts);
        // Cassette interception (mirrors run_completion, for the embeddings seam).
        let decision =
            CASSETTE.with(|c| c.borrow().as_ref().map(|cass| cass.decide(&cassette_key)));
        let response = match decision {
            Some(crate::cassette::Decision::Replay(entry)) => {
                let resp = EmbedResponse {
                    embeddings: entry.embeddings,
                    model: entry.model.clone(),
                    usage: Usage {
                        prompt_tokens: entry.prompt_tokens,
                        model: entry.model,
                        ..Default::default()
                    },
                };
                span.set_dispatch("cassette", &req_model);
                span.set_response(&sema_otel::ResponseFacts {
                    input_tokens: resp.usage.prompt_tokens,
                    output_tokens: 0,
                    response_model: resp.model.clone(),
                    ..Default::default()
                });
                resp
            }
            Some(crate::cassette::Decision::Miss(k)) => return Err(cassette_miss_error(&k)),
            other => {
                let recording = matches!(other, Some(crate::cassette::Decision::Record));
                let resp = with_embedding_provider(|p| {
                    let resp = match p.embed(request) {
                        Ok(r) => r,
                        Err(e) => {
                            span.record_error(llm_error_kind(&e), &e.to_string());
                            return Err(SemaError::Llm(e.to_string()));
                        }
                    };
                    span.set_dispatch(p.name(), &req_model);
                    span.set_response(&sema_otel::ResponseFacts {
                        input_tokens: resp.usage.prompt_tokens,
                        output_tokens: 0,
                        response_model: resp.model.clone(),
                        cost_usd: pricing::calculate_cost_for(p.name(), &resp.usage),
                        ..Default::default()
                    });
                    Ok(resp)
                })?;
                if recording {
                    CASSETTE.with(|c| {
                        if let Some(cass) = c.borrow_mut().as_mut() {
                            cass.record_entry(crate::cassette::TapeEntry::from_embed(
                                &cassette_key,
                                &resp.model,
                                &resp.embeddings,
                                resp.usage.prompt_tokens,
                            ));
                        }
                    });
                }
                resp
            }
        };

        track_usage(&response.usage)?;
        Ok(embed_value_from_response(&response, single))
    });

    // (llm/rerank query documents {:top-k 5 :model "..." :provider :cohere})
    // Cross-encoder reranking. Returns a list of {:index :score :document}, highest
    // relevance first. `documents` is a list of strings.
    register_fn(env, "llm/rerank", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/rerank", "2-3", args.len()));
        }
        let query = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string query", args[0].type_name()))?
            .to_string();
        let documents: Vec<String> = args[1]
            .as_seq()
            .ok_or_else(|| SemaError::type_error("list of strings", args[1].type_name()))?
            .iter()
            .map(|d| {
                d.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| SemaError::type_error("string document", d.type_name()))
            })
            .collect::<Result<_, _>>()?;
        if documents.is_empty() {
            return Ok(Value::list(vec![]));
        }

        let mut top_k = None;
        let mut model = None;
        let mut provider = None;
        if let Some(opts) = args.get(2).and_then(|v| v.as_map_rc()) {
            top_k = get_opt_u32(&opts, "top-k").map(|n| n as usize);
            model = get_opt_string(&opts, "model");
            provider = opts
                .get(&Value::keyword("provider"))
                .and_then(|p| p.as_keyword().or_else(|| p.as_str().map(|s| s.to_string())));
        }

        // OpenInference RERANKER span (no-op unless telemetry + compat are on).
        let span = sema_otel::reranker_span(&query, model.as_deref().unwrap_or(""), top_k);
        span.set_input(&documents);

        let request = RerankRequest {
            query,
            documents: documents.clone(),
            top_k,
            model,
        };
        let resp = with_rerank_provider(provider.as_deref(), |p| {
            p.rerank(request).map_err(|e| {
                span.record_error(llm_error_kind(&e), &e.to_string());
                SemaError::Llm(e.to_string())
            })
        })?;

        // Record reordered output (content + scores) on the span.
        let out_docs: Vec<(String, f64)> = resp
            .results
            .iter()
            .filter_map(|r| documents.get(r.index).map(|d| (d.clone(), r.score)))
            .collect();
        span.set_output(&out_docs);

        let out: Vec<Value> = resp
            .results
            .iter()
            .map(|r| {
                let mut m = BTreeMap::new();
                m.insert(Value::keyword("index"), Value::int(r.index as i64));
                m.insert(Value::keyword("score"), Value::float(r.score));
                m.insert(
                    Value::keyword("document"),
                    Value::string(documents.get(r.index).map(|s| s.as_str()).unwrap_or("")),
                );
                Value::map(m)
            })
            .collect();
        Ok(Value::list(out))
    });

    // (llm/similarity vec1 vec2) — cosine similarity
    register_fn(env, "llm/similarity", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/similarity", "2", args.len()));
        }

        let a_is_bv = args[0].as_bytevector().is_some();
        let b_is_bv = args[1].as_bytevector().is_some();
        let a_is_list = args[0].as_seq().is_some();
        let b_is_list = args[1].as_seq().is_some();

        if a_is_bv && b_is_bv {
            let ba = args[0].as_bytevector().unwrap();
            let bb = args[1].as_bytevector().unwrap();
            if ba.len() != bb.len() {
                return Err(SemaError::eval(format!(
                    "llm/similarity: bytevectors must have same length ({} vs {})",
                    ba.len(),
                    bb.len()
                )));
            }
            if ba.is_empty() {
                return Err(SemaError::eval("llm/similarity: empty vectors"));
            }
            if ba.len() % 8 != 0 {
                return Err(SemaError::eval(format!(
                    "llm/similarity: bytevector length must be a multiple of 8 (got {})",
                    ba.len()
                )));
            }
            let mut dot = 0.0_f64;
            let mut mag_a = 0.0_f64;
            let mut mag_b = 0.0_f64;
            for (ca, cb) in ba.chunks_exact(8).zip(bb.chunks_exact(8)) {
                let fa = f64::from_le_bytes(ca.try_into().unwrap());
                let fb = f64::from_le_bytes(cb.try_into().unwrap());
                dot += fa * fb;
                mag_a += fa * fa;
                mag_b += fb * fb;
            }
            if mag_a == 0.0 || mag_b == 0.0 {
                Ok(Value::float(0.0))
            } else {
                Ok(Value::float(dot / (mag_a.sqrt() * mag_b.sqrt())))
            }
        } else if a_is_list && b_is_list {
            let va = extract_float_vec(&args[0])?;
            let vb = extract_float_vec(&args[1])?;
            if va.len() != vb.len() {
                return Err(SemaError::eval(format!(
                    "llm/similarity: vectors must have same length ({} vs {})",
                    va.len(),
                    vb.len()
                )));
            }
            if va.is_empty() {
                return Err(SemaError::eval("llm/similarity: empty vectors"));
            }
            let mut dot = 0.0_f64;
            let mut mag_a = 0.0_f64;
            let mut mag_b = 0.0_f64;
            for i in 0..va.len() {
                dot += va[i] * vb[i];
                mag_a += va[i] * va[i];
                mag_b += vb[i] * vb[i];
            }
            if mag_a == 0.0 || mag_b == 0.0 {
                Ok(Value::float(0.0))
            } else {
                Ok(Value::float(dot / (mag_a.sqrt() * mag_b.sqrt())))
            }
        } else {
            Err(SemaError::eval(
                "llm/similarity: both arguments must be the same type (both bytevectors or both lists). \
                 Use embedding/->list or embedding/list->embedding to convert between formats.",
            ))
        }
    });

    register_fn(env, "embedding/length", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("embedding/length", "1", args.len()));
        }
        let bv = args[0]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
        if bv.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "embedding/length: bytevector length {} is not divisible by 8",
                bv.len()
            )));
        }
        Ok(Value::int((bv.len() / 8) as i64))
    });

    register_fn(env, "embedding/ref", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("embedding/ref", "2", args.len()));
        }
        let bv = args[0]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
        let idx = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?;
        if bv.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "embedding/ref: bytevector length {} is not divisible by 8",
                bv.len()
            )));
        }
        let idx = idx as usize;
        let num_elements = bv.len() / 8;
        if idx >= num_elements {
            return Err(SemaError::eval(format!(
                "embedding/ref: index {} out of bounds (length {})",
                idx, num_elements
            )));
        }
        let start = idx * 8;
        let bytes: [u8; 8] = bv[start..start + 8].try_into().unwrap();
        Ok(Value::float(f64::from_le_bytes(bytes)))
    });

    register_fn(env, "embedding/->list", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("embedding/->list", "1", args.len()));
        }
        let bv = args[0]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
        if bv.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "embedding/->list: bytevector length {} is not divisible by 8",
                bv.len()
            )));
        }
        let floats: Vec<Value> = bv
            .chunks_exact(8)
            .map(|chunk| {
                let bytes: [u8; 8] = chunk.try_into().unwrap();
                Value::float(f64::from_le_bytes(bytes))
            })
            .collect();
        Ok(Value::list(floats))
    });

    register_fn(env, "embedding/list->embedding", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity(
                "embedding/list->embedding",
                "1",
                args.len(),
            ));
        }
        let items = args[0]
            .as_seq()
            .ok_or_else(|| SemaError::type_error("list or vector", args[0].type_name()))?;
        let mut bytes = Vec::with_capacity(items.len() * 8);
        for (i, item) in items.iter().enumerate() {
            let f = item.as_float().ok_or_else(|| {
                SemaError::eval(format!(
                    "embedding/list->embedding: element {} is {}, expected number",
                    i,
                    item.type_name()
                ))
            })?;
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        Ok(Value::bytevector(bytes))
    });

    register_fn(env, "llm/reset-usage", |_args| {
        SESSION_USAGE.with(|u| *u.borrow_mut() = Usage::default());
        LAST_USAGE.with(|u| *u.borrow_mut() = None);
        SESSION_COST.with(|sc| *sc.borrow_mut() = 0.0);
        Ok(Value::nil())
    });

    // Type predicates for LLM types
    register_fn(env, "prompt?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("prompt?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_prompt_rc().is_some()))
    });

    register_fn(env, "message?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("message?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_message_rc().is_some()))
    });

    // (message/with-image :user "Describe this" bytevec)
    // (message/with-image :user "Describe this" bytevec {:media-type "image/png"})
    register_fn(env, "message/with-image", |args| {
        if args.len() < 3 || args.len() > 4 {
            return Err(SemaError::arity("message/with-image", "3-4", args.len()));
        }
        let role = if let Some(kw) = args[0].as_keyword() {
            match kw.as_str() {
                "system" => Role::System,
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                other => {
                    return Err(SemaError::eval(format!(
                        "message/with-image: unknown role '{other}'"
                    )))
                }
            }
        } else {
            return Err(SemaError::type_error("keyword", args[0].type_name()));
        };
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();
        let bv = args[2]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[2].type_name()))?;

        let media_type = if let Some(opts) = args.get(3).and_then(|v| v.as_map_rc()) {
            opts.get(&Value::keyword("media-type"))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| detect_media_type(bv).to_string())
        } else {
            detect_media_type(bv).to_string()
        };

        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(bv);

        Ok(Value::message(Message {
            role,
            content: text,
            images: vec![ImageAttachment { data, media_type }],
        }))
    });

    register_fn(env, "conversation?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_conversation_rc().is_some()))
    });

    register_fn(env, "tool?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_tool_def_rc().is_some()))
    });

    register_fn(env, "agent?", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent?", "1", args.len()));
        }
        Ok(Value::bool(args[0].as_agent_rc().is_some()))
    });

    // Tool accessor functions
    register_fn(env, "tool/name", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool/name", "1", args.len()));
        }
        let t = args[0]
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", args[0].type_name()))?;
        Ok(Value::string(&t.name))
    });

    register_fn(env, "tool/description", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool/description", "1", args.len()));
        }
        let t = args[0]
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", args[0].type_name()))?;
        Ok(Value::string(&t.description))
    });

    register_fn(env, "tool/parameters", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("tool/parameters", "1", args.len()));
        }
        let t = args[0]
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", args[0].type_name()))?;
        Ok(t.parameters.clone())
    });

    // (agent {:system "…" :tools […] :model "…" :max-turns N}) — build an anonymous,
    // reusable actor value (system prompt + tools + model + max-turns) without binding
    // it. The named form is `defagent`; this is the plain constructor used inline (e.g.
    // `(define bot (agent {:tools [t]}))` or passed to a `step` via `:agent`). All opts
    // are optional; the name is empty for an anonymous agent (a `:agent` step falls back
    // to the role label "step" when the name is empty). Mirrors `register_agent`'s opts
    // extraction in sema-eval, the path `defagent` uses.
    register_fn(env, "agent", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent", "1", args.len()));
        }
        let opts = args[0]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
        let system = opts
            .get(&Value::keyword("system"))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let tools = opts
            .get(&Value::keyword("tools"))
            .map(|v| {
                if let Some(l) = v.as_list() {
                    l.to_vec()
                } else if let Some(v) = v.as_vector() {
                    v.to_vec()
                } else {
                    vec![]
                }
            })
            .unwrap_or_default();
        let max_turns = opts
            .get(&Value::keyword("max-turns"))
            .and_then(|v| v.as_int())
            .unwrap_or(10) as usize;
        let model = opts
            .get(&Value::keyword("model"))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let name = opts
            .get(&Value::keyword("name"))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        Ok(Value::agent(Agent {
            name,
            system,
            tools,
            max_turns,
            model,
        }))
    });

    // Agent accessor functions
    register_fn(env, "agent/name", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/name", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::string(&a.name))
    });

    register_fn(env, "agent/system", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/system", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::string(&a.system))
    });

    register_fn(env, "agent/tools", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/tools", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::list(a.tools.clone()))
    });

    register_fn(env, "agent/model", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/model", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::string(&a.model))
    });

    register_fn(env, "agent/max-turns", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("agent/max-turns", "1", args.len()));
        }
        let a = args[0]
            .as_agent_rc()
            .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
        Ok(Value::int(a.max_turns as i64))
    });

    // (conversation/add-message conv :role "content")
    register_fn(env, "conversation/add-message", |args| {
        if args.len() != 3 {
            return Err(SemaError::arity(
                "conversation/add-message",
                "3",
                args.len(),
            ));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let role_kw = args[1]
            .as_keyword()
            .ok_or_else(|| SemaError::type_error("keyword", args[1].type_name()))?;
        let role = match role_kw.as_str() {
            "system" => Role::System,
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            other => {
                return Err(SemaError::eval(format!(
                    "conversation/add-message: unknown role '{other}'"
                )))
            }
        };
        let content = args[2]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[2].to_string());
        let mut new_messages = conv.messages.clone();
        new_messages.push(Message {
            role,
            content,
            images: Vec::new(),
        });
        Ok(Value::conversation(Conversation {
            messages: new_messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/model conv) — get the model name
    register_fn(env, "conversation/model", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/model", "1", args.len()));
        }
        let c = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        Ok(Value::string(&c.model))
    });

    // (conversation/system conv) — get the system message content, or nil
    register_fn(env, "conversation/system", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/system", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        Ok(conv
            .messages
            .iter()
            .find(|m| m.role == Role::System)
            .map(|m| Value::string(&m.content))
            .unwrap_or_else(Value::nil))
    });

    // (conversation/set-system conv "new system message") — set/replace the system message
    register_fn(env, "conversation/set-system", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("conversation/set-system", "2", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let new_system = args[1]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[1].to_string());
        let mut messages: Vec<Message> = conv
            .messages
            .iter()
            .filter(|m| m.role != Role::System)
            .cloned()
            .collect();
        messages.insert(
            0,
            Message {
                role: Role::System,
                content: new_system,
                images: Vec::new(),
            },
        );
        Ok(Value::conversation(Conversation {
            messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/filter conv pred) — keep only messages where (pred msg) is truthy
    register_fn_ctx(env, "conversation/filter", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("conversation/filter", "2", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let pred = &args[1];
        let mut filtered = Vec::new();
        for msg in &conv.messages {
            let msg_val = Value::message(msg.clone());
            let result = sema_core::call_callback(ctx, pred, &[msg_val])?;
            if result.is_truthy() {
                filtered.push(msg.clone());
            }
        }
        Ok(Value::conversation(Conversation {
            messages: filtered,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/map conv f) — transform each message with (f msg), returns list of results
    register_fn_ctx(env, "conversation/map", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("conversation/map", "2", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let func = &args[1];
        let mut results = Vec::new();
        for msg in &conv.messages {
            let msg_val = Value::message(msg.clone());
            let result = sema_core::call_callback(ctx, func, &[msg_val])?;
            results.push(result);
        }
        Ok(Value::list(results))
    });

    // (conversation/say-as conv system-prompt "message" opts?) — say with a different system prompt for one turn
    register_fn(env, "conversation/say-as", |args| {
        if args.len() < 3 || args.len() > 4 {
            return Err(SemaError::arity("conversation/say-as", "3-4", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;

        // Second arg: either a prompt value (use its system messages) or a string
        let system_override = if let Some(p) = args[1].as_prompt_rc() {
            p.messages
                .iter()
                .filter(|m| m.role == Role::System)
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        } else if let Some(s) = args[1].as_str() {
            s.to_string()
        } else {
            return Err(SemaError::type_error(
                "prompt or string",
                args[1].type_name(),
            ));
        };

        let user_msg = args[2]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| args[2].to_string());

        // Parse optional opts
        let mut temperature = None;
        let mut max_tokens = None;
        if let Some(opts_val) = args.get(3) {
            if let Some(opts) = opts_val.as_map_rc() {
                temperature = get_opt_f64(&opts, "temperature");
                max_tokens = get_opt_u32(&opts, "max-tokens");
            }
        }

        // Build messages for API call — use the system override instead of any existing system msg
        let mut chat_messages: Vec<ChatMessage> = conv
            .messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
            .collect();
        chat_messages.push(ChatMessage::new("user", user_msg.clone()));

        let mut request = ChatRequest::new(conv.model.clone(), chat_messages);
        request.temperature = temperature;
        request.max_tokens = max_tokens.or(Some(4096));
        request.system = Some(system_override);

        let response = do_complete(request)?;
        track_usage(&response.usage)?;

        // Build new conversation preserving the original system message (not the override)
        let mut new_messages = conv.messages.clone();
        new_messages.push(Message {
            role: Role::User,
            content: user_msg,
            images: Vec::new(),
        });
        new_messages.push(Message {
            role: Role::Assistant,
            content: response.content,
            images: Vec::new(),
        });

        let mut metadata = conv.metadata.clone();
        accumulate_usage(&mut metadata, &response.usage);
        Ok(Value::conversation(Conversation {
            messages: new_messages,
            model: conv.model.clone(),
            metadata,
        }))
    });

    // (conversation/token-count conv) — count total tokens in conversation messages
    register_fn(env, "conversation/token-count", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity(
                "conversation/token-count",
                "1",
                args.len(),
            ));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        // Approximate: ~4 chars per token (common heuristic)
        let total_chars: usize = conv.messages.iter().map(|m| m.content.len()).sum();
        let estimated_tokens = (total_chars as f64 / 4.0).ceil() as i64;
        Ok(Value::int(estimated_tokens))
    });

    // (conversation/cost conv) — cumulative cost in USD, summed from each turn's actual
    // usage as it was sent (see accumulate_usage in conversation/say). Returns nil when no
    // priced turn has been recorded.
    register_fn(env, "conversation/cost", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/cost", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        match conv
            .metadata
            .get("usage-cost")
            .and_then(|s| s.parse::<f64>().ok())
        {
            Some(cost) => Ok(Value::float(cost)),
            None => Ok(Value::nil()),
        }
    });

    // (prompt/fill prompt vars-map) — substitute {{key}} in all message contents
    register_fn(env, "prompt/fill", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/fill", "2", args.len()));
        }
        let p = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let vars = args[1]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        let messages: Vec<Message> = p
            .messages
            .iter()
            .map(|m| {
                let filled = fill_template(&m.content, &vars);
                Message {
                    role: m.role.clone(),
                    content: filled,
                    images: m.images.clone(),
                }
            })
            .collect();
        Ok(Value::prompt(Prompt { messages }))
    });

    // (prompt/slots prompt) — return list of unfilled {{slot}} names
    register_fn(env, "prompt/slots", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("prompt/slots", "1", args.len()));
        }
        let p = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let mut slots = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for m in &p.messages {
            let mut chars = m.content.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '{' && chars.peek() == Some(&'{') {
                    chars.next();
                    let mut name = String::new();
                    let mut found_close = false;
                    while let Some(c) = chars.next() {
                        if c == '}' && chars.peek() == Some(&'}') {
                            chars.next();
                            found_close = true;
                            break;
                        }
                        name.push(c);
                    }
                    if found_close && !name.is_empty() && seen.insert(name.clone()) {
                        slots.push(Value::keyword(&name));
                    }
                }
            }
        }
        Ok(Value::list(slots))
    });

    // ---- Conversation inspection (issue #12, Part 3) ----

    // (conversation/length conv) — number of messages
    register_fn(env, "conversation/length", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/length", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        Ok(Value::int(conv.messages.len() as i64))
    });

    // (conversation/turns conv) — number of assistant replies (user/assistant exchanges)
    register_fn(env, "conversation/turns", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/turns", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let turns = conv
            .messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .count();
        Ok(Value::int(turns as i64))
    });

    // (conversation/models-used conv) — list of models (the conversation carries one)
    register_fn(env, "conversation/models-used", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity(
                "conversation/models-used",
                "1",
                args.len(),
            ));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        if conv.model.is_empty() {
            Ok(Value::list(Vec::new()))
        } else {
            Ok(Value::list(vec![Value::string(&conv.model)]))
        }
    });

    // (conversation/stats conv) — aggregate report. Token/cost figures come from the
    // real usage accumulated by conversation/say (see the usage-* metadata written there);
    // they are 0 / nil when no priced turn has been sent.
    register_fn(env, "conversation/stats", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("conversation/stats", "1", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let turns = conv
            .messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .count() as i64;
        let prompt_tokens: i64 = conv
            .metadata
            .get("usage-prompt-tokens")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let completion_tokens: i64 = conv
            .metadata
            .get("usage-completion-tokens")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let cost = conv
            .metadata
            .get("usage-cost")
            .and_then(|s| s.parse::<f64>().ok());

        let mut tokens = BTreeMap::new();
        tokens.insert(Value::keyword("prompt"), Value::int(prompt_tokens));
        tokens.insert(Value::keyword("completion"), Value::int(completion_tokens));
        tokens.insert(
            Value::keyword("total"),
            Value::int(prompt_tokens + completion_tokens),
        );

        let models = if conv.model.is_empty() {
            Value::list(Vec::new())
        } else {
            Value::list(vec![Value::string(&conv.model)])
        };

        let mut stats = BTreeMap::new();
        stats.insert(
            Value::keyword("messages"),
            Value::int(conv.messages.len() as i64),
        );
        stats.insert(Value::keyword("turns"), Value::int(turns));
        stats.insert(Value::keyword("tokens"), Value::map(tokens));
        stats.insert(
            Value::keyword("cost"),
            cost.map(Value::float).unwrap_or_else(Value::nil),
        );
        stats.insert(Value::keyword("models"), models);
        Ok(Value::map(stats))
    });

    // ---- Conversation surgery (issue #12, Part 3) ----

    // (conversation/remove conv idx) — drop the message at idx
    register_fn(env, "conversation/remove", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("conversation/remove", "2", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let idx = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        let mut messages = conv.messages.clone();
        if idx < 0 || idx as usize >= messages.len() {
            return Err(SemaError::eval(format!(
                "conversation/remove: index {idx} out of bounds (length {})",
                messages.len()
            )));
        }
        messages.remove(idx as usize);
        Ok(Value::conversation(Conversation {
            messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/insert conv idx msg) | (conversation/insert conv idx :role "content")
    register_fn(env, "conversation/insert", |args| {
        if args.len() < 3 || args.len() > 4 {
            return Err(SemaError::arity("conversation/insert", "3-4", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let idx = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        let msg = message_from_tail(&args[2..], "conversation/insert")?;
        let mut messages = conv.messages.clone();
        // idx == len is allowed (append); anything past that is out of bounds.
        if idx < 0 || idx as usize > messages.len() {
            return Err(SemaError::eval(format!(
                "conversation/insert: index {idx} out of bounds (length {})",
                messages.len()
            )));
        }
        messages.insert(idx as usize, msg);
        Ok(Value::conversation(Conversation {
            messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/replace conv idx msg) | (conversation/replace conv idx :role "content")
    register_fn(env, "conversation/replace", |args| {
        if args.len() < 3 || args.len() > 4 {
            return Err(SemaError::arity("conversation/replace", "3-4", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let idx = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        let msg = message_from_tail(&args[2..], "conversation/replace")?;
        let mut messages = conv.messages.clone();
        if idx < 0 || idx as usize >= messages.len() {
            return Err(SemaError::eval(format!(
                "conversation/replace: index {idx} out of bounds (length {})",
                messages.len()
            )));
        }
        messages[idx as usize] = msg;
        Ok(Value::conversation(Conversation {
            messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // (conversation/map-role conv :role f) — transform only messages of `role` with (f msg),
    // which must return a message; other messages pass through unchanged.
    register_fn_ctx(env, "conversation/map-role", |ctx, args| {
        if args.len() != 3 {
            return Err(SemaError::arity("conversation/map-role", "3", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let role = parse_role(&args[1], "conversation/map-role")?;
        let func = &args[2];
        let mut messages = Vec::with_capacity(conv.messages.len());
        for msg in &conv.messages {
            if msg.role == role {
                let result = sema_core::call_callback(ctx, func, &[Value::message(msg.clone())])?;
                let new_msg = result
                    .as_message_rc()
                    .ok_or_else(|| SemaError::type_error("message", result.type_name()))?;
                messages.push((*new_msg).clone());
            } else {
                messages.push(msg.clone());
            }
        }
        Ok(Value::conversation(Conversation {
            messages,
            model: conv.model.clone(),
            metadata: conv.metadata.clone(),
        }))
    });

    // ---- Conversation search (issue #12, Part 3) ----

    // (conversation/search conv query) — case-insensitive substring search over message
    // content; returns a list of {:index :role :content} maps for each hit.
    register_fn(env, "conversation/search", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("conversation/search", "2", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let query = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_lowercase();
        let mut hits = Vec::new();
        for (i, m) in conv.messages.iter().enumerate() {
            if m.content.to_lowercase().contains(&query) {
                let mut hit = BTreeMap::new();
                hit.insert(Value::keyword("index"), Value::int(i as i64));
                hit.insert(Value::keyword("role"), Value::keyword(&m.role.to_string()));
                hit.insert(Value::keyword("content"), Value::string(&m.content));
                hits.push(Value::map(hit));
            }
        }
        Ok(Value::list(hits))
    });

    // (conversation/find conv pred) — first message where (pred msg) is truthy, else nil
    register_fn_ctx(env, "conversation/find", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("conversation/find", "2", args.len()));
        }
        let conv = args[0]
            .as_conversation_rc()
            .ok_or_else(|| SemaError::type_error("conversation", args[0].type_name()))?;
        let pred = &args[1];
        for m in &conv.messages {
            let msg_val = Value::message(m.clone());
            if sema_core::call_callback(ctx, pred, std::slice::from_ref(&msg_val))?.is_truthy() {
                return Ok(msg_val);
            }
        }
        Ok(Value::nil())
    });

    // ---- Prompt algebra (issue #12, Part 7) — exact (role, content) matching ----

    // (prompt/diff a b) — {:added [msgs only in b] :removed [msgs only in a]}
    register_fn(env, "prompt/diff", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/diff", "2", args.len()));
        }
        let a = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let b = args[1]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[1].type_name()))?;
        let a_keys: Vec<_> = a.messages.iter().map(msg_key).collect();
        let b_keys: Vec<_> = b.messages.iter().map(msg_key).collect();
        let added: Vec<Value> = b
            .messages
            .iter()
            .filter(|m| !a_keys.contains(&msg_key(m)))
            .map(|m| Value::message(m.clone()))
            .collect();
        let removed: Vec<Value> = a
            .messages
            .iter()
            .filter(|m| !b_keys.contains(&msg_key(m)))
            .map(|m| Value::message(m.clone()))
            .collect();
        let mut out = BTreeMap::new();
        out.insert(Value::keyword("added"), Value::list(added));
        out.insert(Value::keyword("removed"), Value::list(removed));
        Ok(Value::map(out))
    });

    // (prompt/union a b) — messages of a then b, de-duplicated, order preserved
    register_fn(env, "prompt/union", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/union", "2", args.len()));
        }
        let a = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let b = args[1]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[1].type_name()))?;
        let mut seen: Vec<(Role, String)> = Vec::new();
        let mut messages = Vec::new();
        for m in a.messages.iter().chain(b.messages.iter()) {
            let key = (m.role.clone(), m.content.clone());
            if !seen.contains(&key) {
                seen.push(key);
                messages.push(m.clone());
            }
        }
        Ok(Value::prompt(Prompt { messages }))
    });

    // (prompt/intersection a b) — messages present in both (order/dedup from a)
    register_fn(env, "prompt/intersection", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/intersection", "2", args.len()));
        }
        let a = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let b = args[1]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[1].type_name()))?;
        let b_keys: Vec<_> = b.messages.iter().map(msg_key).collect();
        let mut seen: Vec<(Role, String)> = Vec::new();
        let mut messages = Vec::new();
        for m in &a.messages {
            let key = (m.role.clone(), m.content.clone());
            if b_keys.contains(&msg_key(m)) && !seen.contains(&key) {
                seen.push(key);
                messages.push(m.clone());
            }
        }
        Ok(Value::prompt(Prompt { messages }))
    });

    // (prompt/difference a b) — messages in a but not b (order/dedup from a)
    register_fn(env, "prompt/difference", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("prompt/difference", "2", args.len()));
        }
        let a = args[0]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[0].type_name()))?;
        let b = args[1]
            .as_prompt_rc()
            .ok_or_else(|| SemaError::type_error("prompt", args[1].type_name()))?;
        let b_keys: Vec<_> = b.messages.iter().map(msg_key).collect();
        let mut seen: Vec<(Role, String)> = Vec::new();
        let mut messages = Vec::new();
        for m in &a.messages {
            let key = (m.role.clone(), m.content.clone());
            if !b_keys.contains(&msg_key(m)) && !seen.contains(&key) {
                seen.push(key);
                messages.push(m.clone());
            }
        }
        Ok(Value::prompt(Prompt { messages }))
    });

    // (llm/set-default :provider-name) — switch the active provider
    register_fn(env, "llm/set-default", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("llm/set-default", "1", args.len()));
        }
        let name = args[0]
            .as_keyword()
            .or_else(|| args[0].as_str().map(|s| s.to_string()))
            .ok_or_else(|| SemaError::type_error("keyword or string", args[0].type_name()))?;
        PROVIDER_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            if reg.get(&name).is_some() {
                reg.set_default(&name);
                Ok(Value::keyword(&name))
            } else {
                Err(SemaError::Llm(format!("provider not configured: {name}")))
            }
        })
    });

    // (llm/list-providers) — list configured providers
    register_fn(env, "llm/list-providers", |_args| {
        PROVIDER_REGISTRY.with(|reg| {
            let reg = reg.borrow();
            let names: Vec<Value> = reg
                .provider_names()
                .into_iter()
                .map(|n| Value::keyword(&n))
                .collect();
            Ok(Value::list(names))
        })
    });

    // (llm/current-provider) — query active provider/model
    register_fn(env, "llm/current-provider", |_args| {
        PROVIDER_REGISTRY.with(|reg| {
            let reg = reg.borrow();
            match reg.default_provider() {
                Some(p) => {
                    let mut map = BTreeMap::new();
                    map.insert(Value::keyword("name"), Value::keyword(p.name()));
                    map.insert(Value::keyword("model"), Value::string(p.default_model()));
                    Ok(Value::map(map))
                }
                None => Ok(Value::nil()),
            }
        })
    });

    // (llm/pricing-status)
    register_fn(env, "llm/pricing-status", |_args| {
        let (source, updated_at) = pricing::pricing_status();
        let mut map = std::collections::BTreeMap::new();
        map.insert(Value::keyword("source"), Value::symbol(source));
        if let Some(date) = updated_at {
            map.insert(Value::keyword("updated-at"), Value::string(&date));
        }
        Ok(Value::map(map))
    });

    // (llm/set-budget max-cost-usd) — set a budget limit
    register_fn(env, "llm/set-budget", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("llm/set-budget", "1", args.len()));
        }
        let max_cost = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        crate::builtins::set_budget(max_cost);
        Ok(Value::nil())
    });

    // (llm/clear-budget) — clear the budget limit
    register_fn(env, "llm/clear-budget", |_args| {
        crate::builtins::clear_budget();
        Ok(Value::nil())
    });

    // (llm/budget-remaining) — query budget status
    register_fn(env, "llm/budget-remaining", |_args| {
        let Some(frame) = active_budget() else {
            return Ok(Value::nil());
        };
        let f = frame.borrow();
        if f.cost_limit.is_none() && f.token_limit.is_none() {
            return Ok(Value::nil());
        }
        let mut map = BTreeMap::new();
        if let Some(max_cost) = f.cost_limit {
            let spent = f.cost_spent;
            map.insert(Value::keyword("limit"), Value::float(max_cost));
            map.insert(Value::keyword("spent"), Value::float(spent));
            map.insert(Value::keyword("remaining"), Value::float(max_cost - spent));
        }
        if let Some(max_tokens) = f.token_limit {
            let tokens_spent = f.tokens_spent;
            map.insert(Value::keyword("token-limit"), Value::int(max_tokens as i64));
            map.insert(
                Value::keyword("tokens-spent"),
                Value::int(tokens_spent as i64),
            );
            map.insert(
                Value::keyword("tokens-remaining"),
                Value::int((max_tokens.saturating_sub(tokens_spent)) as i64),
            );
        }
        Ok(Value::map(map))
    });

    // (llm/with-budget {:max-cost-usd 0.50 :max-tokens 10000} thunk)
    register_fn_ctx(env, "llm/with-budget", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/with-budget", "2", args.len()));
        }
        let opts = args[0]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
        let body_fn = &args[1];
        if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", body_fn.type_name()));
        }

        let max_cost = opts
            .get(&Value::keyword("max-cost-usd"))
            .and_then(|v| v.as_float());
        let max_tokens = opts
            .get(&Value::keyword("max-tokens"))
            .and_then(|v| v.as_int())
            .map(|v| v.max(0) as u64);

        if max_cost.is_none() && max_tokens.is_none() {
            return Err(SemaError::eval(
                "llm/with-budget: requires at least :max-cost-usd or :max-tokens",
            ));
        }

        // `:on-stream :pre-gate` opts streaming calls into budget enforcement (checked
        // before opening the stream). Default `:off` keeps streams unenforced.
        let pregate = opts
            .get(&Value::keyword("on-stream"))
            .and_then(|v| v.as_keyword())
            .map(|s| s == "pre-gate")
            .unwrap_or(false);

        push_budget_scope(max_cost, max_tokens);
        let prev_pregate = STREAM_BUDGET_PREGATE.with(|c| c.replace(pregate));
        let result = sema_core::call_callback(ctx, body_fn, &[]);
        STREAM_BUDGET_PREGATE.with(|c| c.set(prev_pregate));
        pop_budget_scope();
        result
    });

    // --- Cache builtins ---

    register_fn(env, "llm/cache-key", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/cache-key", "1-2", args.len()));
        }
        let prompt = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let mut model = String::new();
        let mut temperature = None;
        let mut system = None;
        if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
            model = get_opt_string(&opts, "model").unwrap_or_default();
            temperature = get_opt_f64(&opts, "temperature");
            system = get_opt_string(&opts, "system");
        }
        let messages = vec![ChatMessage::new("user", prompt)];
        let mut request = ChatRequest::new(model, messages);
        request.temperature = temperature;
        request.system = system;
        Ok(Value::string(&compute_cache_key(&request)))
    });

    register_fn(env, "llm/cache-clear", |_args| {
        let mem_count = CACHE_MEM.with(|c| {
            let mut cache = c.borrow_mut();
            let count = cache.len();
            cache.clear();
            count
        });
        let dir = cache_dir();
        if dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if entry
                        .path()
                        .extension()
                        .map(|e| e == "json")
                        .unwrap_or(false)
                    {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
        CACHE_HITS.with(|c| c.set(0));
        CACHE_MISSES.with(|c| c.set(0));
        Ok(Value::int(mem_count as i64))
    });

    register_fn(env, "llm/cache-stats", |_args| {
        let hits = CACHE_HITS.with(|c| c.get());
        let misses = CACHE_MISSES.with(|c| c.get());
        let size = CACHE_MEM.with(|c| c.borrow().len());
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("hits"), Value::int(hits as i64));
        map.insert(Value::keyword("misses"), Value::int(misses as i64));
        map.insert(Value::keyword("size"), Value::int(size as i64));
        Ok(Value::map(map))
    });

    register_fn_ctx(env, "llm/with-cache", |ctx, args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/with-cache", "1-2", args.len()));
        }
        let (body_fn, ttl) = if args.len() == 2 {
            let opts = args[0]
                .as_map_rc()
                .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
            let ttl = get_opt_u32(&opts, "ttl").unwrap_or(3600) as i64;
            (&args[1], ttl)
        } else {
            (&args[0], 3600i64)
        };
        if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", body_fn.type_name()));
        }
        let prev_enabled = CACHE_ENABLED.with(|c| c.get());
        let prev_ttl = CACHE_TTL_SECS.with(|c| c.get());
        CACHE_ENABLED.with(|c| c.set(true));
        CACHE_TTL_SECS.with(|c| c.set(ttl));
        let result = sema_core::call_callback(ctx, body_fn, &[]);
        CACHE_ENABLED.with(|c| c.set(prev_enabled));
        CACHE_TTL_SECS.with(|c| c.set(prev_ttl));
        result
    });

    // --- Cassette (record/replay) builtins ---

    register_fn_ctx(env, "llm/with-cassette", |ctx, args| {
        // (llm/with-cassette "path.jsonl" [{:mode :auto}] thunk)
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/with-cassette", "2 or 3", args.len()));
        }
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let (mode, body_fn) = if args.len() == 3 {
            let opts = args[1]
                .as_map_ref()
                .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
            let mode = get_opt_effort(opts, "mode")
                .map(|s| crate::cassette::CassetteMode::parse(&s))
                .unwrap_or(crate::cassette::CassetteMode::Auto);
            (mode, &args[2])
        } else {
            (crate::cassette::CassetteMode::Auto, &args[1])
        };
        if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", body_fn.type_name()));
        }
        let cassette = crate::cassette::Cassette::load(std::path::PathBuf::from(path), mode);
        // Swap in the cassette and disable the response cache for the dynamic extent
        // (a cache hit would short-circuit before the tape — see crate::cassette).
        let prev_cassette = CASSETTE.with(|c| c.borrow_mut().replace(cassette));
        let prev_cache = CACHE_ENABLED.with(|c| c.replace(false));
        let result = sema_core::call_callback(ctx, body_fn, &[]);
        // Flush the tape, then restore the prior cassette + cache state.
        CASSETTE.with(|c| {
            if let Some(cass) = c.borrow().as_ref() {
                let _ = cass.save();
            }
        });
        CASSETTE.with(|c| *c.borrow_mut() = prev_cassette);
        CACHE_ENABLED.with(|c| c.set(prev_cache));
        result
    });

    register_fn(env, "llm/cassette-load", |args| {
        // (llm/cassette-load "path" [{:mode :replay}]) — install globally.
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/cassette-load", "1 or 2", args.len()));
        }
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let mode = if args.len() == 2 {
            let opts = args[1]
                .as_map_ref()
                .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
            get_opt_effort(opts, "mode")
                .map(|s| crate::cassette::CassetteMode::parse(&s))
                .unwrap_or(crate::cassette::CassetteMode::Auto)
        } else {
            crate::cassette::CassetteMode::Auto
        };
        let cassette = crate::cassette::Cassette::load(std::path::PathBuf::from(path), mode);
        CASSETTE.with(|c| *c.borrow_mut() = Some(cassette));
        Ok(Value::nil())
    });

    register_fn(env, "llm/cassette-save", |_args| {
        let saved = CASSETTE.with(|c| c.borrow().as_ref().map(|cass| cass.save()));
        match saved {
            Some(Ok(())) => Ok(Value::bool(true)),
            Some(Err(e)) => Err(SemaError::eval(format!("cassette save failed: {e}"))),
            None => Ok(Value::bool(false)),
        }
    });

    register_fn(env, "llm/cassette-eject", |_args| {
        let cass = CASSETTE.with(|c| c.borrow_mut().take());
        if let Some(cass) = cass {
            let _ = cass.save();
            Ok(Value::bool(true))
        } else {
            Ok(Value::bool(false))
        }
    });

    // --- Fallback provider builtins ---

    register_fn_ctx(env, "llm/with-fallback", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/with-fallback", "2", args.len()));
        }
        let providers = args[0]
            .as_seq()
            .ok_or_else(|| SemaError::type_error("list or vector", args[0].type_name()))?;
        let body_fn = &args[1];
        if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", body_fn.type_name()));
        }
        let chain: Vec<FallbackEntry> = providers
            .iter()
            .map(parse_fallback_entry)
            .collect::<Result<_, _>>()?;
        let prev = FALLBACK_CHAIN.with(|c| c.borrow().clone());
        FALLBACK_CHAIN.with(|c| *c.borrow_mut() = Some(chain));
        let result = sema_core::call_callback(ctx, body_fn, &[]);
        FALLBACK_CHAIN.with(|c| *c.borrow_mut() = prev);
        result
    });

    register_fn(env, "llm/providers", |_args| {
        let names = PROVIDER_REGISTRY.with(|reg| reg.borrow().provider_names());
        Ok(Value::list(
            names.into_iter().map(|n| Value::keyword(&n)).collect(),
        ))
    });

    register_fn(env, "llm/default-provider", |_args| {
        let name = PROVIDER_REGISTRY.with(|reg| {
            reg.borrow()
                .default_provider()
                .map(|p| p.name().to_string())
        });
        match name {
            Some(n) => Ok(Value::keyword(&n)),
            None => Ok(Value::nil()),
        }
    });

    // --- Token counting builtins ---

    register_fn(env, "llm/token-count", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("llm/token-count", "1", args.len()));
        }
        let char_count = if let Some(s) = args[0].as_str() {
            s.len()
        } else if let Some(list) = args[0].as_seq() {
            list.iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.len())
                        .unwrap_or_else(|| v.to_string().len())
                })
                .sum()
        } else {
            args[0].to_string().len()
        };
        Ok(Value::int((char_count / 4) as i64))
    });

    register_fn(env, "llm/token-estimate", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("llm/token-estimate", "1", args.len()));
        }
        let char_count = if let Some(s) = args[0].as_str() {
            s.len()
        } else {
            args[0].to_string().len()
        };
        let tokens = (char_count / 4) as i64;
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("tokens"), Value::int(tokens));
        map.insert(Value::keyword("method"), Value::string("chars/4"));
        map.insert(Value::keyword("chars"), Value::int(char_count as i64));
        Ok(Value::map(map))
    });

    // --- Vector store builtins ---

    register_fn(env, "vector-store/create", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("vector-store/create", "1", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        VECTOR_STORES.with(|s| s.borrow_mut().insert(name.to_string(), VectorStore::new()));
        Ok(Value::string(name))
    });

    register_fn(env, "vector-store/add", |args| {
        if args.len() != 4 {
            return Err(SemaError::arity("vector-store/add", "4", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let id = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let emb = args[2]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[2].type_name()))?;
        if emb.len() % 8 != 0 {
            return Err(SemaError::eval(format!(
                "vector-store/add: embedding length {} not multiple of 8",
                emb.len()
            )));
        }
        let metadata = args[3].clone();
        VECTOR_STORES.with(|s| {
            let mut s = s.borrow_mut();
            let store = s
                .get_mut(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            store.add(VectorDocument {
                id: id.to_string(),
                embedding: emb.to_vec(),
                metadata,
            });
            Ok(Value::string(id))
        })
    });

    register_fn(env, "vector-store/search", |args| {
        if args.len() != 3 {
            return Err(SemaError::arity("vector-store/search", "3", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let query = args[1]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[1].type_name()))?;
        let k = args[2]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[2].type_name()))?
            as usize;
        // OpenInference RETRIEVER span (no-op unless telemetry + compat are on).
        let span = sema_otel::retriever_span(query.len() / 8, k);
        VECTOR_STORES.with(|s| {
            let s = s.borrow();
            let store = s
                .get(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            let results = store.search(query, k).inspect_err(|e| {
                span.record_error("retrieval_error", &e.to_string());
            })?;
            // (id, content, score) for the span — content pulled from metadata :text/:content.
            let docs: Vec<(String, String, f64)> = results
                .iter()
                .map(|r| (r.id.clone(), metadata_text(&r.metadata), r.score))
                .collect();
            span.set_documents(&docs);
            Ok(Value::list(results.iter().map(|r| r.to_value()).collect()))
        })
    });

    register_fn(env, "vector-store/delete", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("vector-store/delete", "2", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let id = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        VECTOR_STORES.with(|s| {
            let mut s = s.borrow_mut();
            let store = s
                .get_mut(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            Ok(Value::bool(store.delete(id)))
        })
    });

    register_fn(env, "vector-store/count", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("vector-store/count", "1", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        VECTOR_STORES.with(|s| {
            let s = s.borrow();
            let store = s
                .get(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            Ok(Value::int(store.count() as i64))
        })
    });

    // (vector-store/save name) or (vector-store/save name path)
    register_fn(env, "vector-store/save", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("vector-store/save", "1-2", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let explicit_path = args.get(1).and_then(|v| v.as_str()).map(|s| s.to_string());
        VECTOR_STORES.with(|s| {
            let s = s.borrow();
            let store = s
                .get(name)
                .ok_or_else(|| SemaError::eval(format!("vector store '{}' not found", name)))?;
            let path = explicit_path
                .as_deref()
                .or(store.path.as_deref())
                .ok_or_else(|| {
                    SemaError::eval(
                        "vector-store/save: no path associated. Use (vector-store/save name path)",
                    )
                })?;
            let data = store.to_json().map_err(SemaError::Io)?;
            let tmp = format!("{path}.tmp");
            std::fs::write(&tmp, &data)
                .map_err(|e| SemaError::Io(format!("vector-store/save: {e}")))?;
            std::fs::rename(&tmp, path)
                .map_err(|e| SemaError::Io(format!("vector-store/save: {e}")))?;
            Ok(Value::string(path))
        })
    });

    // (vector-store/open name path) — load from disk or create empty, associate path
    register_fn(env, "vector-store/open", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("vector-store/open", "2", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let path = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let mut store = if std::path::Path::new(path).exists() {
            let data = std::fs::read(path)
                .map_err(|e| SemaError::Io(format!("vector-store/open: {e}")))?;
            VectorStore::from_json(&data)
                .map_err(|e| SemaError::Io(format!("vector-store/open: {e}")))?
        } else {
            VectorStore::new()
        };
        store.path = Some(path.to_string());
        VECTOR_STORES.with(|s| s.borrow_mut().insert(name.to_string(), store));
        Ok(Value::string(name))
    });

    // --- Vector math builtins ---

    register_fn(env, "vector/cosine-similarity", |args| {
        let (a, b) = require_matching_bytevectors("vector/cosine-similarity", args)?;
        let (mut dot, mut ma, mut mb) = (0.0_f64, 0.0_f64, 0.0_f64);
        for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
            let (fa, fb) = (
                f64::from_le_bytes(ca.try_into().unwrap()),
                f64::from_le_bytes(cb.try_into().unwrap()),
            );
            dot += fa * fb;
            ma += fa * fa;
            mb += fb * fb;
        }
        Ok(Value::float(if ma == 0.0 || mb == 0.0 {
            0.0
        } else {
            dot / (ma.sqrt() * mb.sqrt())
        }))
    });

    register_fn(env, "vector/dot-product", |args| {
        let (a, b) = require_matching_bytevectors("vector/dot-product", args)?;
        let mut dot = 0.0_f64;
        for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
            dot += f64::from_le_bytes(ca.try_into().unwrap())
                * f64::from_le_bytes(cb.try_into().unwrap());
        }
        Ok(Value::float(dot))
    });

    register_fn(env, "vector/normalize", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("vector/normalize", "1", args.len()));
        }
        let bv = args[0]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
        if bv.is_empty() || bv.len() % 8 != 0 {
            return Err(SemaError::eval("vector/normalize: invalid bytevector"));
        }
        let floats: Vec<f64> = bv
            .chunks_exact(8)
            .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let mag: f64 = floats.iter().map(|f| f * f).sum::<f64>().sqrt();
        let out: Vec<u8> = if mag == 0.0 {
            floats.iter().flat_map(|_| 0.0_f64.to_le_bytes()).collect()
        } else {
            floats
                .iter()
                .flat_map(|f| (f / mag).to_le_bytes())
                .collect()
        };
        Ok(Value::bytevector(out))
    });

    register_fn(env, "vector/distance", |args| {
        let (a, b) = require_matching_bytevectors("vector/distance", args)?;
        let mut sum_sq = 0.0_f64;
        for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
            let d = f64::from_le_bytes(ca.try_into().unwrap())
                - f64::from_le_bytes(cb.try_into().unwrap());
            sum_sq += d * d;
        }
        Ok(Value::float(sum_sq.sqrt()))
    });

    // --- Rate limiting ---

    register_fn_ctx(env, "llm/with-rate-limit", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/with-rate-limit", "2", args.len()));
        }
        let rps = args[0]
            .as_float()
            .or_else(|| args[0].as_int().map(|i| i as f64))
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let body_fn = &args[1];
        if body_fn.as_lambda_rc().is_none() && body_fn.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", body_fn.type_name()));
        }
        let prev = RATE_LIMIT_RPS.with(|r| r.get());
        RATE_LIMIT_RPS.with(|r| r.set(Some(rps)));
        let result = sema_core::call_callback(ctx, body_fn, &[]);
        RATE_LIMIT_RPS.with(|r| r.set(prev));
        result
    });

    // --- Convenience wrappers ---

    register_fn(env, "llm/summarize", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/summarize", "1-2", args.len()));
        }
        let text = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

        let mut model = String::new();
        let mut max_length: Option<u32> = None;
        let mut style = "paragraph".to_string();

        if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
            model = get_opt_string(&opts, "model").unwrap_or_default();
            max_length = get_opt_u32(&opts, "max-length");
            if let Some(s) = get_opt_string(&opts, "style") {
                style = s;
            }
        }

        let style_instruction = match style.as_str() {
            "bullet-points" | "bullets" => "Use bullet points.",
            "one-line" => "Respond with a single sentence summary.",
            _ => "Write a concise paragraph summary.",
        };
        let length_instruction = match max_length {
            Some(n) => format!(" Keep the summary under {} words.", n),
            None => String::new(),
        };
        let system =
            format!("Summarize the following text. {style_instruction}{length_instruction}");

        let messages = vec![ChatMessage::new("user", text)];
        let mut request = ChatRequest::new(model, messages);
        request.system = Some(system);
        request.max_tokens = Some(4096);

        let response = do_complete(request)?;
        track_usage(&response.usage)?;
        Ok(Value::string(&response.content))
    });

    register_fn(env, "llm/compare", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("llm/compare", "2-3", args.len()));
        }
        let text_a = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let text_b = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

        let mut model = String::new();
        if let Some(opts) = args.get(2).and_then(|v| v.as_map_rc()) {
            model = get_opt_string(&opts, "model").unwrap_or_default();
        }

        let system =
            "Compare the following two texts. Respond with ONLY a JSON object containing:\n\
            - \"similarity\": a number from 0.0 (completely different) to 1.0 (identical)\n\
            - \"differences\": a list of key differences\n\
            - \"summary\": a brief comparison summary\n\
            Do not include any other text."
                .to_string();

        let user_msg = format!("Text A:\n{text_a}\n\nText B:\n{text_b}");
        let messages = vec![ChatMessage::new("user", &user_msg)];
        let mut request = ChatRequest::new(model, messages);
        request.system = Some(system);

        let response = do_complete(request)?;
        track_usage(&response.usage)?;

        let content = response.content.trim();
        let json_str = if content.starts_with("```") {
            content
                .trim_start_matches("```json")
                .trim_start_matches("```")
                .trim_end_matches("```")
                .trim()
        } else {
            content
        };
        let json: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
            SemaError::Llm(format!(
                "failed to parse comparison JSON: {e}\nResponse: {content}"
            ))
        })?;
        Ok(sema_core::json_to_value(&json))
    });

    // (llm/io-sleep-once id [ms]) — AwaitIo spike leaf (NOT for production use).
    //
    // Mimics `llm/chat-once` but does a timer instead of an HTTP call: spawns a
    // `tokio::time::sleep` on the I/O pool and yields `AwaitIo`, so the
    // scheduler parks the task and runs siblings. Proves real overlap across the
    // per-task-VM scheduler before any agent-loop work. Resolves to `id`.
    #[cfg(not(target_arch = "wasm32"))]
    register_fn_ctx(env, "llm/io-sleep-once", |_ctx, args| {
        use std::sync::atomic::Ordering;

        // Vestigial under CALL_NATIVE: the response arrives via the scheduler's
        // `replace_stack_top`, not by re-invoking this native. Kept for symmetry
        // with the shipped `async/await` pattern.
        if let Some(v) = sema_core::take_resume_value() {
            return Ok(v);
        }
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("llm/io-sleep-once", "1-2", args.len()));
        }
        let id = args[0].as_int().unwrap_or(0);
        let ms = args.get(1).and_then(|v| v.as_int()).unwrap_or(1000).max(0) as u64;

        let (tx, mut rx) = tokio::sync::oneshot::channel::<i64>();

        // Bump in-flight + peak on spawn so the test can prove simultaneity.
        let prev = IO_INFLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        IO_PEAK.fetch_max(prev, Ordering::SeqCst);

        let _abort = sema_io::io_spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            // Clamp at 0: a stray decrement from an abandoned future (timeout/pool
            // error-path) must not push a later test's live count negative.
            let _ = IO_INFLIGHT
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some((v - 1).max(0)));
            let _ = tx.send(id);
            // Wake the parked VM thread so it re-polls promptly.
            sema_core::notify_io_complete();
        });

        let handle = Rc::new(sema_core::IoHandle::new(move || {
            use tokio::sync::oneshot::error::TryRecvError;
            match rx.try_recv() {
                Err(TryRecvError::Empty) => sema_core::IoPoll::Pending,
                Ok(v) => sema_core::IoPoll::Ready(Ok(Value::int(v))),
                Err(TryRecvError::Closed) => {
                    sema_core::IoPoll::Ready(Err("io-sleep-once: worker dropped".to_string()))
                }
            }
        }));
        sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
        Ok(Value::nil())
    });
}

fn require_matching_bytevectors<'a>(
    name: &str,
    args: &'a [Value],
) -> Result<(&'a [u8], &'a [u8]), SemaError> {
    if args.len() != 2 {
        return Err(SemaError::arity(name, "2", args.len()));
    }
    let a = args[0]
        .as_bytevector()
        .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
    let b = args[1]
        .as_bytevector()
        .ok_or_else(|| SemaError::type_error("bytevector", args[1].type_name()))?;
    if a.len() != b.len() {
        return Err(SemaError::eval(format!(
            "{name}: length mismatch ({} vs {})",
            a.len() / 8,
            b.len() / 8
        )));
    }
    if a.is_empty() || a.len() % 8 != 0 {
        return Err(SemaError::eval(format!(
            "{name}: invalid bytevector length {}",
            a.len()
        )));
    }
    Ok((a, b))
}

fn extract_float_vec(val: &Value) -> Result<Vec<f64>, SemaError> {
    let items = val
        .as_seq()
        .ok_or_else(|| SemaError::type_error("list of numbers", val.type_name()))?;
    items
        .iter()
        .map(|v| {
            v.as_float()
                .ok_or_else(|| SemaError::type_error("number", v.type_name()))
        })
        .collect()
}

fn complete_with_prompt(prompt: &Prompt, opts: Option<&Value>) -> Result<Value, SemaError> {
    let messages: Vec<ChatMessage> = prompt
        .messages
        .iter()
        .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
        .collect();

    let mut model = String::new();
    let mut max_tokens = None;
    let mut temperature = None;

    if let Some(opts) = opts.and_then(|v| v.as_map_rc()) {
        model = get_opt_string(&opts, "model").unwrap_or_default();
        max_tokens = get_opt_u32(&opts, "max-tokens");
        temperature = get_opt_f64(&opts, "temperature");
    }

    let mut request = ChatRequest::new(model, messages);
    request.max_tokens = max_tokens.or(Some(4096));
    request.temperature = temperature;
    request.timeout_ms = opt_timeout_ms(opts);

    // Per-call observability tags/metadata (read inside do_complete's span).
    let _tele = install_call_telemetry(opts.and_then(|v| v.as_map_rc()).as_ref());
    let response = do_complete(request)?;
    track_usage(&response.usage)?;
    Ok(Value::string(&response.content))
}

fn message_to_chat_message(m: &Message) -> ChatMessage {
    if m.images.is_empty() {
        ChatMessage::new(m.role.to_string(), m.content.clone())
    } else {
        let mut blocks = Vec::new();
        for img in &m.images {
            blocks.push(ContentBlock::Image {
                media_type: Some(img.media_type.clone()),
                data: img.data.clone(),
            });
        }
        blocks.push(ContentBlock::Text {
            text: m.content.clone(),
        });
        ChatMessage::with_blocks(m.role.to_string(), blocks)
    }
}

fn extract_messages(val: &Value) -> Result<Vec<ChatMessage>, SemaError> {
    if let Some(items) = val.as_seq() {
        let mut messages = Vec::new();
        for item in items.iter() {
            let m = item
                .as_message_rc()
                .ok_or_else(|| SemaError::type_error("message", item.type_name()))?;
            messages.push(message_to_chat_message(&m));
        }
        Ok(messages)
    } else if let Some(p) = val.as_prompt_rc() {
        Ok(p.messages.iter().map(message_to_chat_message).collect())
    } else {
        Err(SemaError::type_error(
            "list of messages or prompt",
            val.type_name(),
        ))
    }
}

fn format_reask_prompt(prev_response: &str, errors: &str, schema_desc: &str) -> String {
    format!(
        "Your previous response did not match the required schema.\n\n\
         Previous response:\n```json\n{prev_response}\n```\n\n\
         Validation errors:\n{errors}\n\n\
         Please respond with ONLY a corrected JSON object matching this schema:\n\
         {schema_desc}\nDo not include any other text."
    )
}

fn format_schema(val: &Value) -> String {
    if let Some(map) = val.as_map_rc() {
        let mut fields = Vec::new();
        for (k, v) in map.iter() {
            let key = k
                .as_keyword()
                .or_else(|| k.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| k.to_string());
            let type_str = if let Some(inner) = v.as_map_rc() {
                if let Some(t) = inner.get(&Value::keyword("type")) {
                    t.as_keyword()
                        .or_else(|| t.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| t.to_string())
                } else {
                    "any".to_string()
                }
            } else {
                "any".to_string()
            };
            fields.push(format!("  \"{key}\": <{type_str}>"));
        }
        format!("{{\n{}\n}}", fields.join(",\n"))
    } else {
        val.to_string()
    }
}

/// Validate that an extracted Sema value matches the expected schema.
/// The schema is a map of keyword keys to field descriptors (maps with `:type`).
/// Returns Ok(()) if valid, or Err with a description of mismatches.
fn validate_extraction(result: &Value, schema: &Value) -> Result<(), String> {
    let schema_map = match schema.as_map_rc() {
        Some(m) => m,
        None => return Ok(()),
    };
    let result_map = match result.as_map_rc() {
        Some(m) => m,
        None => return Err(format!("expected map result, got {}", result.type_name())),
    };

    let mut errors = Vec::new();

    for (key, field_spec) in schema_map.iter() {
        let key_name = key
            .as_keyword()
            .or_else(|| key.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| key.to_string());

        // Check if field is optional (only applies to map-style field specs)
        let is_optional = if let Some(spec) = field_spec.as_map_rc() {
            spec.get(&Value::keyword("optional"))
                .map(|v| v.is_truthy())
                .unwrap_or(false)
        } else {
            false
        };

        let result_val = result_map.get(key);
        match result_val {
            None => {
                if !is_optional {
                    errors.push(format!("missing key: {key_name}"));
                }
            }
            Some(val) => {
                if let Some(spec) = field_spec.as_map_rc() {
                    // Type checking
                    if let Some(type_val) = spec.get(&Value::keyword("type")) {
                        let type_name = type_val
                            .as_keyword()
                            .or_else(|| type_val.as_str().map(|s| s.to_string()))
                            .unwrap_or_else(|| type_val.to_string());
                        let ok = match type_name.as_str() {
                            "string" => val.as_str().is_some(),
                            "number" => val.as_float().is_some(),
                            "boolean" | "bool" => val.as_bool().is_some(),
                            "list" | "array" => val.as_seq().is_some(),
                            _ => true,
                        };
                        if !ok {
                            errors.push(format!(
                                "key {key_name}: expected {type_name}, got {}",
                                val.type_name()
                            ));
                            continue; // skip :validate if type check failed
                        }
                    }

                    // Custom predicate validation via :validate
                    if let Some(validate_fn) = spec.get(&Value::keyword("validate")) {
                        let custom_msg = spec
                            .get(&Value::keyword("message"))
                            .and_then(|v| v.as_str().map(|s| s.to_string()));

                        match sema_core::with_stdlib_ctx(|ctx| {
                            sema_core::call_callback(ctx, validate_fn, std::slice::from_ref(val))
                        }) {
                            Ok(v) if v.is_truthy() => {} // validation passed
                            Ok(_) => {
                                let msg = custom_msg.unwrap_or_else(|| {
                                    format!("custom validation failed for value {}", val)
                                });
                                errors.push(format!("key {key_name}: {msg}"));
                            }
                            Err(e) => {
                                errors.push(format!("key {key_name}: validation error: {e}"));
                            }
                        }
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn compute_cache_key(request: &ChatRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.model.as_bytes());
    if let Some(temp) = request.temperature {
        hasher.update(temp.to_le_bytes());
    }
    if let Some(ref system) = request.system {
        hasher.update(system.as_bytes());
    }
    for msg in &request.messages {
        hasher.update(msg.role.as_bytes());
        hasher.update(msg.content.to_text().as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn cache_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(".sema")
        .join("cache")
        .join("llm")
}

fn cache_file_path(key: &str) -> std::path::PathBuf {
    cache_dir().join(format!("{key}.json"))
}

fn load_cached(key: &str) -> Option<CachedResponse> {
    let mem_hit = CACHE_MEM.with(|c| c.borrow().get(key).cloned());
    if let Some(cached) = mem_hit {
        return Some(cached);
    }
    let path = cache_file_path(key);
    let data = std::fs::read_to_string(&path).ok()?;
    let cached: CachedResponse = serde_json::from_str(&data).ok()?;
    CACHE_MEM.with(|c| c.borrow_mut().insert(key.to_string(), cached.clone()));
    Some(cached)
}

fn store_cached(key: &str, response: &ChatResponse) {
    let cached = CachedResponse {
        content: response.content.clone(),
        model: response.model.clone(),
        prompt_tokens: response.usage.prompt_tokens,
        completion_tokens: response.usage.completion_tokens,
        cached_at: unix_timestamp(),
    };
    CACHE_MEM.with(|c| c.borrow_mut().insert(key.to_string(), cached.clone()));
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string(&cached) {
        let _ = std::fs::write(cache_file_path(key), json);
    }
}

fn is_cache_valid(cached: &CachedResponse) -> bool {
    let ttl = CACHE_TTL_SECS.with(|c| c.get());
    (unix_timestamp() - cached.cached_at) < ttl
}

/// Send a ChatRequest via the default provider with caching, fallback, and rate-limit retry.
/// Build the OTel `ResponseFacts` snapshot from a served response. Cost is priced as
/// served by `provider` (matches `track_usage`).
fn response_facts(provider: &str, resp: &ChatResponse) -> sema_otel::ResponseFacts {
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
const CONTENT_FIELD_MAX: usize = 8192;

fn truncate_content(s: &str) -> String {
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
fn messages_json(messages: &[ChatMessage]) -> String {
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
fn content_json(role: &str, content: &str) -> String {
    serde_json::json!([{
        "role": role,
        "parts": [{"type": "text", "content": truncate_content(content)}],
    }])
    .to_string()
}

/// Conversation / session / user identity threaded into the agent + completion spans.
#[derive(Default, Clone)]
struct ConvScope {
    conversation: Option<String>,
    session: Option<String>,
    user: Option<String>,
}

impl ConvScope {
    /// Read `:conversation-id` / `:session-id` / `:user-id` from an options map.
    fn from_opts(opts: Option<&Rc<BTreeMap<Value, Value>>>) -> Self {
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
    fn open(&self) -> Option<sema_otel::ConversationGuard> {
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
fn llm_error_kind(e: &crate::types::LlmError) -> &'static str {
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
    static CALL_TAGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Per-call user observability metadata (string -> string), same lifecycle as tags.
    static CALL_META: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// RAII install of per-call user tags/metadata. Saves and restores the previous values
/// on drop so a nested LLM call (e.g. `llm/complete` inside an agent tool) can't wipe an
/// outer call's telemetry.
struct CallTelemetry {
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
fn install_call_telemetry(opts: Option<&Rc<BTreeMap<Value, Value>>>) -> Option<CallTelemetry> {
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
fn apply_call_telemetry_llm(span: &sema_otel::LlmSpan) {
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
fn apply_call_telemetry_agent(span: &sema_otel::AgentSpan) {
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

fn do_complete(request: ChatRequest) -> Result<ChatResponse, SemaError> {
    // Standalone completions get their own conversation id so every chat span carries
    // gen_ai.conversation.id; agent-nested completions inherit the agent's scope.
    let _conv = if sema_otel::current_conversation_id().is_none() {
        Some(sema_otel::set_conversation_scope(
            &sema_otel::new_conversation_id(),
            None,
            None,
        ))
    } else {
        None
    };
    // One CLIENT span per completion. Started here (before cache lookup) so a cache
    // hit still gets a span; request attrs are known up front, provider/model/usage
    // are filled in deeper where they're resolved.
    let span = sema_otel::llm_span("chat");
    span.set_request(
        request.temperature,
        request.max_tokens,
        &request.stop_sequences,
        request.reasoning_effort.as_deref(),
    );
    span.set_output_type(request.json_mode);
    // Advertise the tools available this turn (compat: OpenInference llm.tools.*,
    // Traceloop llm.request.functions.*). Only built when a backend compat is active.
    if sema_otel::compat_active() && !request.tools.is_empty() {
        let views: Vec<sema_otel::ToolView> = request
            .tools
            .iter()
            .map(|t| sema_otel::ToolView {
                name: t.name.clone(),
                description: t.description.clone(),
                json_schema: t.parameters.to_string(),
            })
            .collect();
        span.set_tools(&views);
    }
    // User :tags / :metadata for this call (auto-tags are derived inside the span).
    apply_call_telemetry_llm(&span);
    // Reset the serving-provider stamp so a cache hit (which serves no provider) doesn't
    // inherit a stale name from a prior completion.
    LAST_SERVING_PROVIDER.with(|p| *p.borrow_mut() = None);
    let cache_enabled = CACHE_ENABLED.with(|c| c.get());
    if !cache_enabled {
        return run_completion(request, &span);
    }
    // Compute the cache key from the model the request will *logically* use, but
    // without mutating the request that flows into the fallback loop. Pre-filling
    // `request.model` here would make it non-empty and defeat the per-provider
    // default/override substitution in `do_complete_with_provider` — sending the
    // wrong provider's model id down the chain (the original cache+fallback bug).
    let key_model = if request.model.is_empty() {
        primary_model_for_cache()?
    } else {
        request.model.clone()
    };
    let mut key_request = request.clone();
    key_request.model = key_model;
    let cache_key = compute_cache_key(&key_request);
    if let Some(cached) = load_cached(&cache_key) {
        if is_cache_valid(&cached) {
            CACHE_HITS.with(|c| c.set(c.get() + 1));
            // A cache hit makes no provider call: no tokens are consumed and no
            // money is spent. Report ZERO usage so the caller's `track_usage` does
            // not re-charge session cost or burn the budget for a cached response
            // (the provider never saw this request). The cached token counts live
            // in the on-disk/in-memory entry if ever needed; the live accounting
            // must reflect actual spend.
            let resp = ChatResponse {
                content: cached.content,
                role: "assistant".to_string(),
                model: cached.model,
                tool_calls: vec![],
                usage: Usage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    model: key_request.model.clone(),
                    ..Default::default()
                },
                stop_reason: Some("cache_hit".to_string()),
            };
            // Cache-hit span: no provider served it; tag gen_ai.cache.hit=true with
            // zero usage (matches the zero-usage accounting invariant).
            span.set_dispatch("", &resp.model);
            span.set_response(&response_facts("", &resp));
            return Ok(resp);
        }
    }
    CACHE_MISSES.with(|c| c.set(c.get() + 1));
    let response = run_completion(request, &span)?;
    store_cached(&cache_key, &response);
    Ok(response)
}

/// Streaming counterpart of [`do_complete`] for the agent tool loop's `:on-text`
/// option. Opens the same per-completion `chat` span/scope, but drives
/// `stream_with_dispatch` and delivers each text delta to the Sema `on_text`
/// callback. Returns the assembled [`ChatResponse`] so the loop's tool-call
/// handling and `track_usage` accounting are byte-identical to the non-streaming
/// path. Streaming bypasses the completion cache (like `llm/stream`).
fn do_complete_streaming(
    ctx: &EvalContext,
    request: ChatRequest,
    on_text: &Value,
) -> Result<ChatResponse, SemaError> {
    let _conv = if sema_otel::current_conversation_id().is_none() {
        Some(sema_otel::set_conversation_scope(
            &sema_otel::new_conversation_id(),
            None,
            None,
        ))
    } else {
        None
    };
    let span = sema_otel::llm_span("chat");
    span.set_request(
        request.temperature,
        request.max_tokens,
        &request.stop_sequences,
        request.reasoning_effort.as_deref(),
    );
    span.set_output_type(request.json_mode);
    let mut chunk_cb = |chunk: &str| -> Result<(), crate::types::LlmError> {
        sema_core::call_callback(ctx, on_text, &[Value::string(chunk)])
            .map_err(|e| crate::types::LlmError::Config(e.to_string()))?;
        Ok(())
    };
    stream_with_dispatch(request, &mut chunk_cb, &span)
}

/// Concurrent counterpart to [`do_complete`] + the native's post-call accounting,
/// for the async-scheduler-task path. Mirrors the concurrent `llm/embed` flow,
/// scaled to a completion: it runs the on-VM-thread stage (conv scope, detached
/// `chat` span, request attrs, cache lookup, cassette decision, fallback-chain
/// resolution into `Arc` clones) SYNCHRONOUSLY, then SPAWNS the wire unit
/// (`run_fallback_retry_async`) as an abortable pool future and YIELDS `AwaitIo`
/// so sibling tasks overlap — cancel/timeout runs the handle's abort hook, which
/// drops the in-flight request. All post-call work (span finalize, retry spans,
/// cache store, cassette record, `track_usage`, content→Value) runs in the
/// poller, on the VM thread, when the future lands — because the native is NOT
/// re-invoked on resume.
///
/// `finalize` shapes the per-native return value from the `ChatResponse` (e.g.
/// `Value::string(&resp.content)` for `llm/complete`). It runs in the poller on the
/// VM thread, AFTER `track_usage`, so it may itself do further VM-thread work
/// (e.g. `llm/extract`'s re-ask loop).
///
/// On a cache hit or cassette replay (no provider call) it finalizes the span,
/// accounts ZERO usage, calls `finalize`, and returns WITHOUT yielding (nothing to
/// overlap) — preserving the zero-usage cache-hit accounting invariant.
#[cfg(not(target_arch = "wasm32"))]
fn do_complete_async_yield(
    request: ChatRequest,
    finalize: Box<dyn FnOnce(ChatResponse) -> Result<Value, SemaError>>,
) -> Result<Value, SemaError> {
    use std::sync::atomic::Ordering;

    // Defensive resume-value drain: the scheduler resumes the bytecode after the
    // CALL via `replace_stack_top`, so this native is not re-invoked — but mirror
    // `llm/embed`/`io-sleep-once` and drain any stray resume value.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    // Standalone completions get their own conversation scope (so the chat span
    // carries gen_ai.conversation.id); agent-nested ones inherit. The detached span
    // captures the conversation id at creation, so the guard need only live across
    // span creation below.
    let _conv = if sema_otel::current_conversation_id().is_none() {
        Some(sema_otel::set_conversation_scope(
            &sema_otel::new_conversation_id(),
            None,
            None,
        ))
    } else {
        None
    };

    // DETACHED chat span: parent captured now, finalized in the poller after the
    // yield (when the active-span stack may hold a sibling task's span, so the span
    // must not pop the stack on drop).
    let span = sema_otel::llm_span_detached("chat");
    span.set_request(
        request.temperature,
        request.max_tokens,
        &request.stop_sequences,
        request.reasoning_effort.as_deref(),
    );
    span.set_output_type(request.json_mode);
    if sema_otel::compat_active() && !request.tools.is_empty() {
        let views: Vec<sema_otel::ToolView> = request
            .tools
            .iter()
            .map(|t| sema_otel::ToolView {
                name: t.name.clone(),
                description: t.description.clone(),
                json_schema: t.parameters.to_string(),
            })
            .collect();
        span.set_tools(&views);
    }
    apply_call_telemetry_llm(&span);

    // ── Cache lookup (hit → finalize inline, zero usage, NO yield) ────────
    let cache_enabled = CACHE_ENABLED.with(|c| c.get());
    let cache_key = if cache_enabled {
        let key_model = if request.model.is_empty() {
            primary_model_for_cache()?
        } else {
            request.model.clone()
        };
        let mut key_request = request.clone();
        key_request.model = key_model;
        let key = compute_cache_key(&key_request);
        if let Some(cached) = load_cached(&key) {
            if is_cache_valid(&cached) {
                CACHE_HITS.with(|c| c.set(c.get() + 1));
                // A cache hit made no provider call → ZERO usage (mirrors
                // `do_complete`), so `track_usage` does not recharge or burn budget.
                let resp = ChatResponse {
                    content: cached.content,
                    role: "assistant".to_string(),
                    model: cached.model,
                    tool_calls: vec![],
                    usage: Usage {
                        prompt_tokens: 0,
                        completion_tokens: 0,
                        model: key_request.model.clone(),
                        ..Default::default()
                    },
                    stop_reason: Some("cache_hit".to_string()),
                };
                span.set_dispatch("", &resp.model);
                span.set_response(&response_facts("", &resp));
                drop(span);
                track_usage(&resp.usage)?;
                return finalize(resp);
            }
        }
        // Cache miss (no entry, or entry present but invalid) — mirror the sync
        // `do_complete` so `(llm/cache-stats)` :misses is accurate for async traffic.
        CACHE_MISSES.with(|c| c.set(c.get() + 1));
        Some(key)
    } else {
        None
    };

    // ── Cassette decision (replay → inline, no yield; miss → Err) ─────────
    // Keyed by the request as-is (no default-model resolution), matching
    // `run_completion`'s key so record/replay agree with the sync path.
    let cassette_decision = CASSETTE.with(|c| {
        c.borrow().as_ref().map(|cass| {
            let key = compute_cache_key(&request);
            (key.clone(), cass.decide(&key))
        })
    });
    match cassette_decision {
        Some((_, crate::cassette::Decision::Replay(entry))) => {
            let resp = entry.to_response();
            span.set_dispatch("cassette", &resp.model);
            span.set_response(&response_facts("cassette", &resp));
            drop(span);
            track_usage(&resp.usage)?;
            return finalize(resp);
        }
        Some((k, crate::cassette::Decision::Miss(_))) => return Err(cassette_miss_error(&k)),
        _ => {}
    }
    let cassette_record_key = match cassette_decision {
        Some((k, crate::cassette::Decision::Record)) => Some(k),
        _ => None,
    };

    // ── Resolve the fallback chain (or default provider) into Arc clones ──
    // Done on the VM thread so the offloaded worker touches no thread-locals.
    enforce_rate_limit();
    let max_retries = NETWORK_MAX_RETRIES.with(|c| c.get());
    // Capture the retry-backoff base on the VM thread so the offloaded wire
    // stage honors it (pool workers have their own RETRY_BASE_MS TLS copies) —
    // see `retry_backoff_ms`. Threaded through `run_fallback_retry_async`.
    let retry_base_ms = RETRY_BASE_MS.with(|c| c.get());
    let chain: Vec<ResolvedProvider> = PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let fallback = FALLBACK_CHAIN.with(|c| c.borrow().clone());
        match fallback {
            Some(entries) if !entries.is_empty() => entries
                .iter()
                .map(|e| {
                    reg.get(&e.provider)
                        .map(|p| ResolvedProvider {
                            provider: p,
                            name: e.provider.clone(),
                            model: e.model.clone(),
                        })
                        .ok_or_else(|| {
                            SemaError::Llm(format!("fallback provider '{}' not found", e.provider))
                        })
                })
                .collect::<Result<Vec<_>, _>>(),
            _ => {
                let p = reg.default_provider().ok_or_else(|| {
                    SemaError::Llm(
                        "no LLM provider configured. Use (llm/configure :anthropic \
                         {:api-key ...}) first"
                            .to_string(),
                    )
                })?;
                let name = p.name().to_string();
                Ok(vec![ResolvedProvider {
                    provider: p,
                    name,
                    model: None,
                }])
            }
        }
    })?;

    // ── Offload the wire unit + yield ────────────────────────────────────
    let (tx, mut rx) =
        tokio::sync::oneshot::channel::<Result<CompleteOutcome, crate::types::LlmError>>();
    let req2 = request.clone();
    // NOTE (async-path compat nuance): the wire stage runs on pool workers, so
    // OpenAI's `DROP_TEMPERATURE` self-heal LEARNS into a worker's TLS, not the
    // VM thread's. The WITHIN-call self-heal (400 → drop temperature → retry
    // once) is fully preserved (openai's `complete_future` strips temperature
    // from the retried request explicitly), so every async completion still
    // succeeds; only the cross-call optimization (skip the doomed first request
    // on later calls) is not shared across the VM thread — each async call may
    // pay one extra 400+retry on temperature-rejecting models. Correctness
    // holds; documented as a minor async-path divergence.
    // Bump in-flight + peak on spawn so a test can prove simultaneity (mirrors the
    // io-sleep-once spike instrumentation). The balancing guard is constructed
    // HERE and moved into the future: an abort that lands before the future's
    // first poll still drops the future — and the captured guard with it — so
    // the gauge cannot strand at +1.
    struct InflightGuard;
    impl Drop for InflightGuard {
        fn drop(&mut self) {
            let _ = IO_INFLIGHT
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some((v - 1).max(0)));
        }
    }
    let prev = IO_INFLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
    IO_PEAK.fetch_max(prev, Ordering::SeqCst);
    let inflight = InflightGuard;
    // Offloaded as a SPAWNED POOL FUTURE (the http/shell abort tier), not
    // spawn_blocking: native-async providers are dropped mid-flight on abort —
    // the in-flight request's connection is torn down, no wasted round-trip.
    // Sync-only providers fall back to the admission-controlled blocking tier
    // inside `complete_once_async`, where cancel stays best-effort.
    let abort = sema_io::io_spawn(async move {
        // Balance in-flight on EVERY exit — normal completion or abort-drop.
        let _inflight = inflight;
        let r = run_fallback_retry_async(chain, req2, max_retries, retry_base_ms).await;
        let _ = tx.send(r);
        sema_core::notify_io_complete();
    });

    // Move the span + cassette/cache context INTO the poller closure so all
    // post-call work runs on the VM thread when the future lands.
    let mut span_slot = Some(span);
    let mut finalize_slot = Some(finalize);
    // Capture THIS leaf's per-leaf usage accumulator frame (if a workflow scope is
    // open) the same way the otel span crosses the yield via `span_slot`. Folding into
    // this captured Rc — not whatever frame is on top when the future lands — is what
    // makes async fan-out correct: a sibling leaf opening its own scope can't clobber
    // this in-flight leaf's tally.
    let usage_accum_slot = current_usage_accum();
    // Capture the active BUDGET frame `Rc` the same way (ASYNC-1). The poller runs
    // OUTSIDE the per-task install boundary (during `wake_blocked_tasks`), so the
    // thread-local budget is whatever is active when the future lands — not the frame
    // that was in force when this completion was dispatched. Re-installing THIS captured
    // frame around `track_usage` below is what lets a concurrent `with-budget` fan-out
    // charge one shared aggregate and gate correctly.
    let budget_slot = active_budget();
    let request_for_messages = request;
    // True cancellation: on cancel/timeout the scheduler runs the abort hook,
    // which aborts the spawned wire future → the in-flight provider request is
    // dropped (connection torn down). Never called on normal completion.
    let handle = Rc::new(sema_core::IoHandle::with_abort(
        move || {
            use tokio::sync::oneshot::error::TryRecvError;
            match rx.try_recv() {
                Err(TryRecvError::Empty) => sema_core::IoPoll::Pending,
                Ok(Ok(outcome)) => {
                    let CompleteOutcome {
                        resp,
                        serving_provider,
                        serving_model,
                        retry_events,
                    } = outcome;
                    if let Some(span) = span_slot.take() {
                        // Emit retry spans + set the response facts UNDER this span so
                        // children parent correctly (the detached span is not on the
                        // stack). `entered` installs it as the active parent for the
                        // closure, then restores.
                        span.entered(|| {
                            emit_retry_spans(&retry_events);
                        });
                        span.set_dispatch(&serving_provider, &serving_model);
                        span.set_response(&response_facts(&serving_provider, &resp));
                        span.set_messages(
                            &messages_json(&request_for_messages.messages),
                            &content_json("assistant", &resp.content),
                            request_for_messages
                                .system
                                .as_deref()
                                .map(|s| content_json("system", s))
                                .as_deref(),
                        );
                        // span drops here → ends the span.
                    }
                    set_serving_provider(&serving_provider);
                    if let Some(key) = &cache_key {
                        store_cached(key, &resp);
                    }
                    if let Some(key) = &cassette_record_key {
                        CASSETTE.with(|c| {
                            if let Some(cass) = c.borrow_mut().as_mut() {
                                cass.record_entry(crate::cassette::TapeEntry::from_response(
                                    key, &resp,
                                ));
                            }
                        });
                    }
                    // Fold this completion into the LEAF'S OWN captured accumulator frame —
                    // the `Rc` snapshotted at yield time, not whatever scope is active when
                    // the future lands (the poller runs outside the per-task install
                    // boundary, so the thread-local may now hold a sibling's scope). Price
                    // it the same way `track_usage` does — by the serving provider — then
                    // suppress `track_usage`'s own active-frame fold so this completion is
                    // counted exactly once.
                    if let Some(slot) = &usage_accum_slot {
                        let cost = pricing::calculate_cost_for(&serving_provider, &resp.usage);
                        accumulate_into(slot, &resp.usage, cost);
                    }
                    // Account on the VM thread, then shape the value. A budget overrun
                    // fails the task, exactly as the sync path's `?`. Install THIS
                    // completion's captured budget frame as active around `track_usage` so
                    // the charge + limit check land on the dispatch-time frame (shared by
                    // `Rc` across the fan-out), then restore whatever was active.
                    let track_result = {
                        let prev_budget = ACTIVE_BUDGET
                            .with(|b| std::mem::replace(&mut *b.borrow_mut(), budget_slot.clone()));
                        let r = USAGE_ACCUM_SUPPRESS.with(|s| {
                            s.set(true);
                            let r = track_usage(&resp.usage);
                            s.set(false);
                            r
                        });
                        ACTIVE_BUDGET.with(|b| *b.borrow_mut() = prev_budget);
                        r
                    };
                    if let Err(e) = track_result {
                        return sema_core::IoPoll::Ready(Err(e.to_string()));
                    }
                    let finalize = finalize_slot.take().expect("finalize used once");
                    match finalize(resp) {
                        Ok(value) => sema_core::IoPoll::Ready(Ok(value)),
                        Err(e) => sema_core::IoPoll::Ready(Err(e.to_string())),
                    }
                }
                Ok(Err(e)) => {
                    if let Some(span) = span_slot.take() {
                        span.record_error(llm_error_kind(&e), &e.to_string());
                    }
                    sema_core::IoPoll::Ready(Err(e.to_string()))
                }
                Err(TryRecvError::Closed) => {
                    span_slot.take();
                    sema_core::IoPoll::Ready(Err("complete: io worker dropped".to_string()))
                }
            }
        },
        abort,
    ));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
}

/// Cassette interception seam: below the otel span + response cache (set up in
/// `do_complete`), above the real provider chain (`do_complete_inner`). When a
/// cassette is active it replays a recorded response (still emitting the chat
/// span, populated from the recorded model/usage) or records a fresh one; with no
/// cassette it's a transparent passthrough. See `crate::cassette`.
fn run_completion(
    request: ChatRequest,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    if CASSETTE.with(|c| c.borrow().is_none()) {
        return do_complete_inner(request, span);
    }
    // Key by the request as-is (no default-model resolution) so record and replay
    // produce the same key for an identical call, even with no provider configured
    // (keyless replay). Shares the hashing with the response cache.
    let key = compute_cache_key(&request);
    let decision = CASSETTE.with(|c| c.borrow().as_ref().unwrap().decide(&key));
    match decision {
        crate::cassette::Decision::Replay(entry) => {
            // A replayed call is a stand-in for a real one: emit the span with the
            // recorded facts and let the caller's usage/cost accounting run on the
            // recorded tokens (distinct from a cache hit, which reports zero usage).
            let resp = entry.to_response();
            span.set_dispatch("cassette", &resp.model);
            span.set_response(&response_facts("cassette", &resp));
            Ok(resp)
        }
        crate::cassette::Decision::Miss(k) => Err(cassette_miss_error(&k)),
        crate::cassette::Decision::Record => {
            let resp = do_complete_inner(request, span)?;
            CASSETTE.with(|c| {
                if let Some(cass) = c.borrow_mut().as_mut() {
                    cass.record_entry(crate::cassette::TapeEntry::from_response(&key, &resp));
                }
            });
            Ok(resp)
        }
    }
}

/// The hard error raised on a `:replay`-mode cassette miss (no recorded interaction
/// for this request). Shared by the complete, stream, and embed seams.
fn cassette_miss_error(key: &str) -> SemaError {
    SemaError::Llm(format!(
        "cassette miss in :replay mode (key {key}) — no recorded interaction for this \
         request; re-record the tape or use :auto mode"
    ))
}

/// Streaming counterpart to `run_completion`: replays the recorded chunk sequence
/// (feeding the caller's `on_chunk` so boundaries match) and final response, or
/// records a fresh stream by capturing chunks as they arrive. Transparent
/// passthrough with no active cassette. Sits below the otel span, above the provider.
fn stream_with_cassette(
    p: &dyn LlmProvider,
    request: ChatRequest,
    chunk_cb: &mut dyn FnMut(&str) -> Result<(), crate::types::LlmError>,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    let stream_real = |req: ChatRequest,
                       cb: &mut dyn FnMut(&str) -> Result<(), crate::types::LlmError>|
     -> Result<ChatResponse, SemaError> {
        // Stamp the streaming time-to-first-token on the first chunk delivered by the
        // real provider (mark_first_token is itself idempotent).
        let mut seen_first = false;
        let mut timed = |chunk: &str| -> Result<(), crate::types::LlmError> {
            if !seen_first {
                span.mark_first_token();
                seen_first = true;
            }
            cb(chunk)
        };
        p.stream_complete(req, &mut timed).map_err(|e| {
            span.record_error(llm_error_kind(&e), &e.to_string());
            SemaError::Llm(e.to_string())
        })
    };

    if CASSETTE.with(|c| c.borrow().is_none()) {
        let resp = stream_real(request.clone(), chunk_cb)?;
        span.set_dispatch(p.name(), &request.model);
        span.set_response(&response_facts(p.name(), &resp));
        return Ok(resp);
    }

    let key = compute_cache_key(&request);
    let decision = CASSETTE.with(|c| c.borrow().as_ref().unwrap().decide(&key));
    match decision {
        crate::cassette::Decision::Replay(entry) => {
            for ch in &entry.chunks {
                chunk_cb(ch).map_err(|e| SemaError::Llm(e.to_string()))?;
            }
            let resp = entry.to_response();
            span.set_dispatch("cassette", &resp.model);
            span.set_response(&response_facts("cassette", &resp));
            Ok(resp)
        }
        crate::cassette::Decision::Miss(k) => Err(cassette_miss_error(&k)),
        crate::cassette::Decision::Record => {
            let mut collected: Vec<String> = Vec::new();
            let mut wrap = |chunk: &str| -> Result<(), crate::types::LlmError> {
                collected.push(chunk.to_string());
                chunk_cb(chunk)
            };
            let resp = stream_real(request.clone(), &mut wrap)?;
            CASSETTE.with(|c| {
                if let Some(cass) = c.borrow_mut().as_mut() {
                    cass.record_entry(crate::cassette::TapeEntry::from_stream(
                        &key, &collected, &resp,
                    ));
                }
            });
            span.set_dispatch(p.name(), &request.model);
            span.set_response(&response_facts(p.name(), &resp));
            Ok(resp)
        }
    }
}

/// Cassette key for an embeddings request (model + the input texts).
fn compute_embed_key(request: &EmbedRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"embed");
    if let Some(ref m) = request.model {
        hasher.update(m.as_bytes());
    }
    for t in &request.texts {
        hasher.update(t.as_bytes());
        hasher.update(b"\0");
    }
    format!("{:x}", hasher.finalize())
}

/// Encode an `EmbedResponse`'s vectors into the SAME `Value` the synchronous
/// `llm/embed` returns (single → bytevector; multi → list of bytevectors), so the
/// concurrent (async) and sync paths are byte-identical: both decode through here.
fn embed_value_from_response(resp: &EmbedResponse, single: bool) -> Value {
    if single {
        let embedding = resp.embeddings.first().cloned().unwrap_or_default();
        let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        Value::bytevector(bytes)
    } else {
        Value::list(
            resp.embeddings
                .iter()
                .map(|emb| {
                    let bytes: Vec<u8> = emb.iter().flat_map(|f| f.to_le_bytes()).collect();
                    Value::bytevector(bytes)
                })
                .collect(),
        )
    }
}

/// Resolve the model id used for the cache key when the caller pinned none. With an
/// active fallback chain, the "logical" model is the first chain entry's model (its
/// override if present, else that provider's default); otherwise it's the default
/// provider's default model.
fn primary_model_for_cache() -> Result<String, SemaError> {
    let first_entry =
        FALLBACK_CHAIN.with(|c| c.borrow().as_ref().and_then(|chain| chain.first().cloned()));
    if let Some(entry) = first_entry {
        if let Some(model) = entry.model {
            return Ok(model);
        }
        return PROVIDER_REGISTRY.with(|reg| {
            let reg = reg.borrow();
            reg.get(&entry.provider)
                .map(|p| p.default_model().to_string())
                .ok_or_else(|| {
                    SemaError::Llm(format!("fallback provider '{}' not found", entry.provider))
                })
        });
    }
    with_provider(|p| Ok(p.default_model().to_string()))
}

/// Parameters for `llm/extract`'s validate-and-re-ask stage, captured so the
/// (possibly offloaded) finalize closure can run the synchronous re-ask attempts.
struct ExtractConfig {
    schema: Value,
    schema_desc: String,
    system: String,
    model: String,
    messages: Vec<ChatMessage>,
    validate: bool,
    max_retries: u32,
    reask: bool,
}

/// Parse an LLM extraction response body into a Sema `Value` (strips a ```json
/// fence if present). Shared by every `llm/extract` attempt.
fn extract_parse_response(response: &ChatResponse) -> Result<Value, SemaError> {
    let content = response.content.trim();
    let json_str = if content.starts_with("```") {
        content
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
    } else {
        content
    };
    let json: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        SemaError::Llm(format!(
            "failed to parse LLM JSON response: {e}\nResponse was: {content}"
        ))
    })?;
    Ok(sema_core::json_to_value(&json))
}

/// Validate `llm/extract`'s attempt-0 response and, on validation failure with
/// retries remaining, run the re-ask attempts via the SYNCHRONOUS `do_complete`
/// path (+ `track_usage` per attempt) — preserving the exact loop semantics and
/// error messages of the original native. `first` is the attempt-0 response, which
/// the caller has ALREADY accounted (sync path: inline; async path: in the poller).
fn extract_validate_and_reask(
    first: ChatResponse,
    cfg: &ExtractConfig,
) -> Result<Value, SemaError> {
    let mut last_validation_error = String::new();
    let mut last_response_content = String::new();

    for attempt in 0..=cfg.max_retries {
        // Attempt 0 reuses the already-issued+accounted `first` response; later
        // attempts issue a fresh (re-ask) request synchronously here.
        let response = if attempt == 0 {
            first.clone()
        } else {
            let mut request = ChatRequest::new(cfg.model.clone(), cfg.messages.clone());
            request.json_mode = true;
            request.system = Some(if cfg.reask {
                format_reask_prompt(
                    &last_response_content,
                    &last_validation_error,
                    &cfg.schema_desc,
                )
            } else {
                format!(
                    "{}\n\nYour previous response had validation errors: {}. Please fix.",
                    cfg.system, last_validation_error
                )
            });
            let resp = do_complete(request)?;
            track_usage(&resp.usage)?;
            resp
        };

        let content = response.content.trim().to_string();
        let result = extract_parse_response(&response)?;

        if !cfg.validate {
            return Ok(result);
        }
        match validate_extraction(&result, &cfg.schema) {
            Ok(()) => return Ok(result),
            Err(err) => {
                last_validation_error = err;
                last_response_content = content;
                if attempt == cfg.max_retries {
                    return Err(SemaError::Llm(format!(
                        "extraction validation failed after {} attempt(s): {}",
                        cfg.max_retries + 1,
                        last_validation_error
                    )));
                }
            }
        }
    }

    unreachable!()
}

fn do_complete_inner(
    request: ChatRequest,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    let fallback_chain = FALLBACK_CHAIN.with(|c| c.borrow().clone());
    match fallback_chain {
        Some(chain) if !chain.is_empty() => {
            let mut last_error = None;
            for entry in &chain {
                match do_complete_with_provider(entry, request.clone(), span) {
                    Ok(resp) => return Ok(resp),
                    Err(e) => {
                        eprintln!(
                            "Provider '{}' failed: {}, trying next...",
                            entry.provider, e
                        );
                        last_error = Some(e);
                    }
                }
            }
            let err = last_error.unwrap_or_else(|| SemaError::Llm("all providers failed".into()));
            span.record_error("provider_error", &err.to_string());
            Err(err)
        }
        _ => {
            let r = do_complete_uncached(request, span);
            if let Err(e) = &r {
                span.record_error("provider_error", &e.to_string());
            }
            r
        }
    }
}

thread_local! {
    /// Base delay for exponential backoff between network retries. Tests set this
    /// to 0 via [`set_retry_base_ms`] so retry behavior is asserted on attempt
    /// count without real sleeps.
    static RETRY_BASE_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(500) };
    /// Max same-provider retries on transient errors (429 / 5xx / network).
    static NETWORK_MAX_RETRIES: std::cell::Cell<u32> = const { std::cell::Cell::new(3) };
}

/// Test hook: set the retry backoff base (ms). 0 disables sleeping.
pub fn set_retry_base_ms(ms: u64) {
    RETRY_BASE_MS.with(|c| c.set(ms));
}

/// Test/config hook: set the max number of same-provider network retries.
pub fn set_network_max_retries(n: u32) {
    NETWORK_MAX_RETRIES.with(|c| c.set(n));
}

/// Whether an `LlmError` is worth retrying on the same provider, and the
/// server-suggested wait in ms. `Some(ms)`: retryable — `ms > 0` honors that wait
/// (429 `retry-after`), `ms == 0` means use computed backoff. `None`: not
/// retryable (4xx non-429, parse/config errors).
fn retryable_wait(err: &crate::types::LlmError) -> Option<u64> {
    use crate::types::LlmError::*;
    match err {
        RateLimited { retry_after_ms } => Some(*retry_after_ms),
        // 5xx are transient server faults; network failures and timeouts surface
        // as Http(_). Both are safe to retry.
        Api { status, .. } if *status >= 500 => Some(0),
        Http(_) => Some(0),
        _ => None,
    }
}

/// Capped exponential backoff with full jitter. A positive server hint wins.
/// `base_ms` is the configured retry-backoff base, passed in explicitly (NOT read
/// from the `RETRY_BASE_MS` thread-local here) so the async wire stage — which
/// runs on pool worker threads with their own TLS copies — honors the base the
/// VM thread configured (incl. the `set_retry_base_ms(0)` test hook). The VM
/// thread captures the TL value and threads it down.
fn retry_backoff_ms(attempt: u32, server_hint: u64, base_ms: u64) -> u64 {
    const CAP_MS: u64 = 30_000;
    if server_hint > 0 {
        return server_hint.min(CAP_MS);
    }
    let base = base_ms;
    if base == 0 {
        return 0;
    }
    let ceil = base.saturating_mul(1u64 << attempt.min(6)).min(CAP_MS);
    // Full jitter: a uniform-ish value in [0, ceil]. Sub-nanosecond entropy is
    // plenty here — jitter only affects sleep duration, never control flow.
    let entropy = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    entropy % (ceil + 1)
}

/// A single network-retry event, captured as DATA (not emitted as an otel span at
/// the point it happens). The synchronous completion path emits `retry_span`s
/// inline from these; the async path collects them on a worker thread (no otel TLS
/// there) and replays them as spans in the VM-thread poller. Capturing-as-data is
/// what lets both paths share one retry loop with zero telemetry drift.
#[derive(Debug, Clone)]
struct RetryEvent {
    /// 1-based attempt number that triggered the retry (matches `retry_span`).
    attempt: u32,
    /// `llm_error_kind` of the error that triggered the retry.
    kind: &'static str,
    /// The error's display message.
    msg: String,
    /// The backoff actually applied before the retry, in ms.
    wait_ms: u64,
}

/// Run `provider.complete` with retry on transient errors (429 / 5xx / network),
/// using capped exponential backoff with jitter (429 honors `retry-after`),
/// COLLECTING each retry as a [`RetryEvent`] rather than emitting otel spans.
/// Synchronous-path loop (the VM thread; provider `block_on` already returned
/// before the backoff `thread::sleep`); the async wire stage uses the twin
/// [`complete_with_retry_collecting_async`]. Touches NO thread-locals.
fn complete_with_retry_collecting(
    provider: &dyn LlmProvider,
    request: &ChatRequest,
    max_retries: u32,
    base_ms: u64,
) -> Result<(ChatResponse, Vec<RetryEvent>), crate::types::LlmError> {
    let mut attempt = 0u32;
    let mut events = Vec::new();
    loop {
        match provider.complete(request.clone()) {
            Ok(resp) => return Ok((resp, events)),
            Err(e) => match retryable_wait(&e) {
                Some(hint) if attempt < max_retries => {
                    let wait = retry_backoff_ms(attempt, hint, base_ms);
                    events.push(RetryEvent {
                        attempt: attempt + 1,
                        kind: llm_error_kind(&e),
                        msg: e.to_string(),
                        wait_ms: wait,
                    });
                    if wait > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(wait));
                    }
                    attempt += 1;
                }
                _ => return Err(e),
            },
        }
    }
}

/// Emit one `retry_span` child per collected [`RetryEvent`] under the active LLM
/// span. Called on the VM thread (sync path: inline; async path: in the poller)
/// where the otel context is live.
fn emit_retry_spans(events: &[RetryEvent]) {
    for ev in events {
        let rspan = sema_otel::retry_span(ev.attempt);
        rspan.record_error(ev.kind, &ev.msg);
        rspan.set_wait_ms(ev.wait_ms);
    }
}

/// Run `provider.complete` with retry on transient errors (429 / 5xx / network),
/// using capped exponential backoff with jitter (429 honors `retry-after`).
/// Re-expressed on top of [`complete_with_retry_collecting`] so the sync and async
/// paths share one retry loop: this variant emits the collected retries as otel
/// `retry_span` children inline (the sync path's behavior, unchanged).
fn complete_with_retry(
    provider: &dyn LlmProvider,
    request: &ChatRequest,
    max_retries: u32,
) -> Result<ChatResponse, crate::types::LlmError> {
    // Sync path runs on the VM thread, so reading the TL base here is correct.
    let base_ms = RETRY_BASE_MS.with(|c| c.get());
    let (resp, events) = complete_with_retry_collecting(provider, request, max_retries, base_ms)?;
    emit_retry_spans(&events);
    Ok(resp)
}

/// One resolved fallback target for the offloadable wire stage: an `Arc` provider
/// clone (off the thread-local registry, cloned on the VM thread before offload),
/// the provider's registry name, and an optional per-entry model override.
#[cfg(not(target_arch = "wasm32"))]
struct ResolvedProvider {
    provider: std::sync::Arc<dyn LlmProvider>,
    name: String,
    model: Option<String>,
}

/// Result of the offloadable completion wire stage: the response, the name of the
/// provider that served it (for `set_serving_provider` + pricing on the VM thread),
/// and the collected retry events (replayed as spans in the poller).
#[cfg(not(target_arch = "wasm32"))]
struct CompleteOutcome {
    resp: ChatResponse,
    serving_provider: String,
    serving_model: String,
    retry_events: Vec<RetryEvent>,
}

/// One completion attempt for the async wire stage. Providers with a native
/// async path (`complete_future`) are awaited in-place inside the spawned pool
/// future — aborting the task drops the in-flight request (TRUE cancellation,
/// connection torn down). Sync-only providers (the `complete_future` default —
/// e.g. the FakeProvider test double) fall back to an admission-controlled
/// blocking offload, where cancellation stays best-effort (result discarded,
/// call runs to completion on the worker).
#[cfg(not(target_arch = "wasm32"))]
async fn complete_once_async(
    provider: &std::sync::Arc<dyn LlmProvider>,
    request: &ChatRequest,
) -> Result<ChatResponse, crate::types::LlmError> {
    match provider.complete_future(request.clone()) {
        Some(fut) => fut.await,
        None => {
            let p = provider.clone();
            let req = request.clone();
            sema_io::io_offload_blocking(move || p.complete(req)).await
        }
    }
}

/// Async twin of [`complete_with_retry_collecting`] for the spawned wire stage:
/// same retry policy (429/5xx/network retryable with capped exponential
/// backoff + full jitter, 429 honors `retry-after`, 4xx-non-429 fail fast) and
/// the same collected [`RetryEvent`]s, but each attempt goes through
/// [`complete_once_async`] and the backoff is a `tokio::time::sleep` — so an
/// abort during either the attempt or the backoff drops the future instead of
/// stranding a blocking worker. Touches NO thread-locals.
#[cfg(not(target_arch = "wasm32"))]
async fn complete_with_retry_collecting_async(
    provider: &std::sync::Arc<dyn LlmProvider>,
    request: &ChatRequest,
    max_retries: u32,
    base_ms: u64,
) -> Result<(ChatResponse, Vec<RetryEvent>), crate::types::LlmError> {
    let mut attempt = 0u32;
    let mut events = Vec::new();
    loop {
        match complete_once_async(provider, request).await {
            Ok(resp) => return Ok((resp, events)),
            Err(e) => match retryable_wait(&e) {
                Some(hint) if attempt < max_retries => {
                    let wait = retry_backoff_ms(attempt, hint, base_ms);
                    events.push(RetryEvent {
                        attempt: attempt + 1,
                        kind: llm_error_kind(&e),
                        msg: e.to_string(),
                        wait_ms: wait,
                    });
                    if wait > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                    }
                    attempt += 1;
                }
                _ => return Err(e),
            },
        }
    }
}

/// The OFFLOADED wire unit for an async completion: walk the resolved fallback
/// chain (via [`complete_with_retry_collecting_async`], preserving
/// DROP_TEMPERATURE self-heal + network retry), failing over on error. Does NO
/// thread-local access — no cassette, cache, spans, `track_usage`, or
/// `set_serving_provider` (those all stay on the VM thread, in the poller).
/// Runs inside an `io_spawn`ed pool future, so the scheduler's abort hook
/// cancels it mid-flight.
#[cfg(not(target_arch = "wasm32"))]
async fn run_fallback_retry_async(
    chain: Vec<ResolvedProvider>,
    request: ChatRequest,
    max_retries: u32,
    base_ms: u64,
) -> Result<CompleteOutcome, crate::types::LlmError> {
    let mut last_error = None;
    for entry in &chain {
        let mut req = request.clone();
        // Per-provider override wins; else fill the provider default when unpinned
        // (mirrors `do_complete_with_provider` / `do_complete_uncached`).
        if let Some(model) = &entry.model {
            req.model = model.clone();
        } else if req.model.is_empty() {
            req.model = entry.provider.default_model().to_string();
        }
        match complete_with_retry_collecting_async(&entry.provider, &req, max_retries, base_ms)
            .await
        {
            Ok((resp, retry_events)) => {
                return Ok(CompleteOutcome {
                    resp,
                    serving_provider: entry.name.clone(),
                    serving_model: req.model,
                    retry_events,
                });
            }
            Err(e) => {
                eprintln!("Provider '{}' failed: {e}, trying next...", entry.name);
                last_error = Some(e);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| crate::types::LlmError::Config("all providers failed".into())))
}

fn do_complete_with_provider(
    entry: &FallbackEntry,
    mut request: ChatRequest,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = reg.get(&entry.provider).ok_or_else(|| {
            SemaError::Llm(format!("fallback provider '{}' not found", entry.provider))
        })?;
        // A per-provider chain override wins over any model pinned in the call body
        // (so the chain can target a different model per provider); otherwise fall
        // back to the provider's own default when nothing was pinned. Either way each
        // provider receives a model id valid for itself.
        if let Some(model) = &entry.model {
            request.model = model.clone();
        } else if request.model.is_empty() {
            request.model = provider.default_model().to_string();
        }
        let max_retries = NETWORK_MAX_RETRIES.with(|c| c.get());
        let resp = complete_with_retry(&*provider, &request, max_retries)
            .map_err(|e| SemaError::Llm(e.to_string()))?;
        set_serving_provider(&entry.provider);
        // Provider + model + response are all in scope here, before track_usage
        // consumes the serving-provider stamp.
        span.set_dispatch(&entry.provider, &request.model);
        span.set_response(&response_facts(&entry.provider, &resp));
        span.set_messages(
            &messages_json(&request.messages),
            &content_json("assistant", &resp.content),
            request
                .system
                .as_deref()
                .map(|s| content_json("system", s))
                .as_deref(),
        );
        Ok(resp)
    })
}

/// Parsed `llm/stream`-shaped args: the request, the optional callback, and the
/// optional opts map.
type StreamArgs = (
    ChatRequest,
    Option<Value>,
    Option<Rc<BTreeMap<Value, Value>>>,
);

/// Parse `llm/stream`-shaped args — prompt/messages, then an optional callback
/// (any procedure) and an optional opts map in either order — into the
/// `ChatRequest` plus the raw callback/opts. Shared by the blocking native
/// (`__llm-stream-blocking`) and the non-blocking `__stream-begin`, so both
/// paths accept byte-identical calls.
fn parse_stream_args(args: &[Value]) -> Result<StreamArgs, SemaError> {
    if args.is_empty() || args.len() > 3 {
        return Err(SemaError::arity("llm/stream", "1-3", args.len()));
    }

    let messages = if let Some(s) = args[0].as_str() {
        vec![ChatMessage::new("user", s)]
    } else if let Some(p) = args[0].as_prompt_rc() {
        p.messages
            .iter()
            .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
            .collect()
    } else if args[0].as_seq().is_some() {
        extract_messages(&args[0])?
    } else {
        return Err(SemaError::type_error(
            "string, prompt, or messages",
            args[0].type_name(),
        ));
    };

    let mut callback: Option<Value> = None;
    let mut opts_map: Option<Rc<BTreeMap<Value, Value>>> = None;
    for arg in &args[1..] {
        if arg.as_lambda_rc().is_some() || arg.as_native_fn_rc().is_some() {
            callback = Some(arg.clone());
        } else if let Some(m) = arg.as_map_rc() {
            opts_map = Some(m);
        }
    }

    let mut model = String::new();
    let mut max_tokens = None;
    let mut temperature = None;
    let mut system = None;
    if let Some(ref opts) = opts_map {
        model = get_opt_string(opts, "model").unwrap_or_default();
        max_tokens = get_opt_u32(opts, "max-tokens");
        temperature = get_opt_f64(opts, "temperature");
        system = get_opt_string(opts, "system");
    }

    let mut request = ChatRequest::new(model, messages);
    request.max_tokens = max_tokens.or(Some(4096));
    request.temperature = temperature;
    request.system = system;
    Ok((request, callback, opts_map))
}

/// Stream-open budget pre-gate. When `:on-stream :pre-gate` is active, refuse to OPEN a
/// stream if the scope's spend is already at/over the cost or token limit. (A stream's own
/// cost is unknown until it ends, so this is the only honest gate — a single in-flight
/// stream can still push past the cap, but the next call is blocked.)
fn stream_budget_pregate() -> Result<(), SemaError> {
    if !STREAM_BUDGET_PREGATE.with(|c| c.get()) {
        return Ok(());
    }
    let Some(frame) = active_budget() else {
        return Ok(());
    };
    let f = frame.borrow();
    if let Some(limit) = f.cost_limit {
        let spent = f.cost_spent;
        if spent >= limit {
            return Err(SemaError::Llm(format!(
                "budget exceeded: ${spent:.4} of ${limit:.4} limit already spent — \
                 streaming call blocked at open"
            )));
        }
    }
    if let Some(limit) = f.token_limit {
        let spent = f.tokens_spent;
        if spent >= limit {
            return Err(SemaError::Llm(format!(
                "token budget exceeded: {spent} of {limit} tokens already used — \
                 streaming call blocked at open"
            )));
        }
    }
    Ok(())
}

/// Open a stream against one fallback-chain provider (resolving its per-entry model).
fn stream_one_provider(
    entry: &FallbackEntry,
    mut request: ChatRequest,
    chunk_cb: &mut dyn FnMut(&str) -> Result<(), crate::types::LlmError>,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = reg.get(&entry.provider).ok_or_else(|| {
            SemaError::Llm(format!("fallback provider '{}' not found", entry.provider))
        })?;
        if let Some(model) = &entry.model {
            request.model = model.clone();
        } else if request.model.is_empty() {
            request.model = provider.default_model().to_string();
        }
        let resp = stream_with_cassette(&*provider, request, chunk_cb, span)?;
        set_serving_provider(&entry.provider);
        Ok(resp)
    })
}

/// Stream-open dispatch for `llm/stream`: budget pre-gate + rate-limit, then open the
/// stream through the fallback chain. Fails over to the next provider ONLY if a provider
/// errors *before emitting any chunk*; once a chunk is delivered a mid-stream error
/// surfaces (failing over would re-emit the already-delivered partial — see the spike test
/// `spike_mid_stream_failure_behaviour`).
fn stream_with_dispatch(
    request: ChatRequest,
    chunk_cb: &mut dyn FnMut(&str) -> Result<(), crate::types::LlmError>,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    stream_budget_pregate()?;
    enforce_rate_limit();

    let chain = FALLBACK_CHAIN.with(|c| c.borrow().clone());
    match chain {
        Some(chain) if !chain.is_empty() => {
            let mut last_error = None;
            for entry in &chain {
                let mut emitted = false;
                let result = {
                    let mut wrapped = |c: &str| -> Result<(), crate::types::LlmError> {
                        emitted = true;
                        chunk_cb(c)
                    };
                    stream_one_provider(entry, request.clone(), &mut wrapped, span)
                };
                match result {
                    Ok(resp) => return Ok(resp),
                    Err(e) if emitted => {
                        // Mid-stream failure: surface; do NOT fail over (would duplicate).
                        span.record_error("provider_error", &e.to_string());
                        return Err(e);
                    }
                    Err(e) => {
                        eprintln!(
                            "Provider '{}' failed to open stream: {e}, trying next...",
                            entry.provider
                        );
                        last_error = Some(e);
                    }
                }
            }
            let err = last_error.unwrap_or_else(|| SemaError::Llm("all providers failed".into()));
            span.record_error("provider_error", &err.to_string());
            Err(err)
        }
        _ => with_provider(|p| {
            let mut req = request;
            if req.model.is_empty() {
                req.model = p.default_model().to_string();
            }
            stream_with_cassette(p, req, chunk_cb, span)
        }),
    }
}

/// Original do_complete logic (provider dispatch + rate-limit retry).
fn do_complete_uncached(
    mut request: ChatRequest,
    span: &sema_otel::LlmSpan,
) -> Result<ChatResponse, SemaError> {
    enforce_rate_limit();
    let max_retries = NETWORK_MAX_RETRIES.with(|c| c.get());
    with_provider(|p| {
        if request.model.is_empty() {
            request.model = p.default_model().to_string();
        }
        let resp = complete_with_retry(p, &request, max_retries)
            .map_err(|e| SemaError::Llm(e.to_string()))?;
        set_serving_provider(p.name());
        // Capture provider/model/response before track_usage consumes the stamp.
        span.set_dispatch(p.name(), &request.model);
        span.set_response(&response_facts(p.name(), &resp));
        span.set_messages(
            &messages_json(&request.messages),
            &content_json("assistant", &resp.content),
            request
                .system
                .as_deref()
                .map(|s| content_json("system", s))
                .as_deref(),
        );
        Ok(resp)
    })
}

fn enforce_rate_limit() {
    let rps = RATE_LIMIT_RPS.with(|r| r.get());
    if let Some(rps) = rps {
        let min_interval_ms = (1000.0 / rps) as u64;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let last = RATE_LIMIT_LAST.with(|l| l.get());
        // saturating_sub: a backward wall-clock adjustment makes `now < last`,
        // which would panic (debug) or wrap to a huge value (release) on plain
        // subtraction. Treat that as "no wait needed". This sleep runs on the
        // synchronous caller thread (the provider's own block_on has already
        // returned), so it does not stall a shared tokio runtime worker.
        let elapsed = now.saturating_sub(last);
        if last > 0 && elapsed < min_interval_ms {
            let sleep_ms = min_interval_ms - elapsed;
            std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
        }
        let actual_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        RATE_LIMIT_LAST.with(|l| l.set(actual_now));
    }
}

/// Build ToolSchema list from Sema ToolDef values.
fn build_tool_schemas(tools: &[Value]) -> Result<Vec<ToolSchema>, SemaError> {
    let mut schemas = Vec::new();
    for tool in tools {
        let td = tool
            .as_tool_def_rc()
            .ok_or_else(|| SemaError::type_error("tool", tool.type_name()))?;
        let params_json = sema_value_to_json_schema(&td.parameters);
        schemas.push(ToolSchema {
            name: td.name.clone(),
            description: td.description.clone(),
            parameters: params_json,
        });
    }
    Ok(schemas)
}

/// Convert a Sema schema map into a JSON Schema object for the LLM API.
fn sema_value_to_json_schema(val: &Value) -> serde_json::Value {
    if let Some(map) = val.as_map_rc() {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for (k, v) in map.iter() {
            let key = k
                .as_keyword()
                .or_else(|| k.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| k.to_string());
            let prop = if let Some(inner) = v.as_map_rc() {
                let mut prop_obj = serde_json::Map::new();
                if let Some(t) = inner.get(&Value::keyword("type")) {
                    let type_str = t
                        .as_keyword()
                        .or_else(|| t.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| "string".to_string());
                    prop_obj.insert("type".to_string(), serde_json::Value::String(type_str));
                }
                if let Some(d) = inner.get(&Value::keyword("description")) {
                    let desc = d
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| d.to_string());
                    prop_obj.insert("description".to_string(), serde_json::Value::String(desc));
                }
                if let Some(e) = inner.get(&Value::keyword("enum")) {
                    if let Some(items) = e.as_seq() {
                        let vals: Vec<serde_json::Value> = items
                            .iter()
                            .map(|v| {
                                serde_json::Value::String(
                                    v.as_str()
                                        .map(|s| s.to_string())
                                        .or_else(|| v.as_keyword())
                                        .unwrap_or_else(|| v.to_string()),
                                )
                            })
                            .collect();
                        prop_obj.insert("enum".to_string(), serde_json::Value::Array(vals));
                    }
                }
                // Mark as required unless :optional #t
                let optional = inner
                    .get(&Value::keyword("optional"))
                    .map(|v| v.is_truthy())
                    .unwrap_or(false);
                if !optional {
                    required.push(serde_json::Value::String(key.clone()));
                }
                serde_json::Value::Object(prop_obj)
            } else {
                required.push(serde_json::Value::String(key.clone()));
                serde_json::json!({"type": "string"})
            };
            properties.insert(key, prop);
        }
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required
        })
    } else {
        serde_json::json!({"type": "object", "properties": {}})
    }
}

fn sema_list_to_chat_messages(val: &Value) -> Result<Vec<ChatMessage>, SemaError> {
    if val.is_nil() {
        return Ok(Vec::new());
    }
    let items = val
        .as_seq()
        .ok_or_else(|| SemaError::type_error("list of message maps", val.type_name()))?;
    let mut messages = Vec::with_capacity(items.len());
    for item in items.iter() {
        let m = item
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("message map", item.type_name()))?;
        let role = m
            .get(&Value::keyword("role"))
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_default();
        let content = m
            .get(&Value::keyword("content"))
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| v.to_string())
            })
            .unwrap_or_default();
        let mut msg = ChatMessage::new(role, content);
        // Restore tool-call correlation written by chat_messages_to_sema_list so a
        // re-sent history keeps the assistant tool_calls and the tool-result ids.
        if let Some(tcs) = m
            .get(&Value::keyword("tool-calls"))
            .and_then(|v| v.as_seq())
        {
            msg.tool_calls = tcs
                .iter()
                .filter_map(|tc| {
                    let tm = tc.as_map_rc()?;
                    Some(ToolCall {
                        id: tm
                            .get(&Value::keyword("id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        name: tm
                            .get(&Value::keyword("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        arguments: tm
                            .get(&Value::keyword("arguments"))
                            .map(sema_core::value_to_json_lossy)
                            .unwrap_or_else(|| serde_json::json!({})),
                        thought_signature: tm
                            .get(&Value::keyword("thought-signature"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    })
                })
                .collect();
        }
        msg.tool_call_id = m
            .get(&Value::keyword("tool-call-id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        msg.tool_name = m
            .get(&Value::keyword("tool-name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        messages.push(msg);
    }
    Ok(messages)
}

fn chat_messages_to_sema_list(messages: &[ChatMessage]) -> Value {
    let items: Vec<Value> = messages
        .iter()
        .map(|msg| {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("role"), Value::string(&msg.role));
            map.insert(
                Value::keyword("content"),
                Value::string(&msg.content.to_text()),
            );
            // Preserve tool-call correlation so this history re-sends validly on the
            // next turn. Without it, a re-sent assistant tool-call turn loses its
            // tool_calls and the tool result loses its id — providers 400 on the
            // empty tool_use_id / tool_call_id.
            if !msg.tool_calls.is_empty() {
                let tcs: Vec<Value> = msg
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        let mut m = BTreeMap::new();
                        m.insert(Value::keyword("id"), Value::string(&tc.id));
                        m.insert(Value::keyword("name"), Value::string(&tc.name));
                        m.insert(
                            Value::keyword("arguments"),
                            sema_core::json_to_value(&tc.arguments),
                        );
                        // Gemini's opaque thoughtSignature must survive the Sema
                        // round-trip too, or a :messages/:session continuation
                        // re-sends the turn without it and Gemini 400s.
                        if let Some(ref sig) = tc.thought_signature {
                            m.insert(Value::keyword("thought-signature"), Value::string(sig));
                        }
                        Value::map(m)
                    })
                    .collect();
                map.insert(Value::keyword("tool-calls"), Value::list(tcs));
            }
            if let Some(ref id) = msg.tool_call_id {
                map.insert(Value::keyword("tool-call-id"), Value::string(id));
            }
            if let Some(ref name) = msg.tool_name {
                map.insert(Value::keyword("tool-name"), Value::string(name));
            }
            Value::map(map)
        })
        .collect();
    Value::list(items)
}

/// Bound runaway error loops across the agent conversation (mirrors `run_tool_loop`).
const MAX_CONSECUTIVE_TOOL_ERRORS: usize = 5;

/// Per-run state for the non-blocking (async-context) agent loop. Lives in the
/// thread-local `AGENT_RUNS` slab keyed by an integer token handed to Sema, so it
/// survives every inter-round / inter-tool `AwaitIo` park (the slab is on the VM
/// thread; nothing here is `Send` and nothing crosses threads). No `__agent-*`
/// native holds a `RefCell` borrow of the slab across a callback / tool execution /
/// completion yield — each short-borrows to copy inputs out, drops, does the work,
/// then short-borrows again to write back.
struct AgentLoopState {
    messages: Vec<ChatMessage>,
    tools: Vec<Value>,
    tool_schemas: Vec<ToolSchema>,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    system: Option<String>,
    reasoning_effort: Option<String>,
    on_tool_call: Option<Value>,
    on_text: Option<Value>,
    round: usize,
    max_rounds: usize,
    consecutive_errors: usize,
    pending_tool_calls: Vec<ToolCall>,
    last_content: String,
    first_input: String,
    /// Set once the loop should stop (no tool calls, round cap, or consec-error abort).
    done: bool,
    /// Non-empty error message when the run aborted (consecutive tool errors); raised
    /// by `__agent-finish` so the abort surfaces to the caller like the blocking path.
    abort_error: Option<String>,
    /// Whether a final plain-assistant message has been appended to `messages`.
    final_pushed: bool,
    output_conv_id: String,
    has_opts: bool,
    memory_handle: Option<Value>,
    pre_user_count: usize,
    agent_model: String,
    /// The attached agent OTel span (pushed on the thread-local stack in `__agent-begin`,
    /// popped+ended when this state is removed from the slab). `Option` so the custom
    /// `Drop` can forget it when the otel thread-locals are already gone (see below).
    agent_span: Option<sema_otel::AgentSpan>,
    conv_guard: Option<sema_otel::ConversationGuard>,
    /// The scheduler task this run's driver loop executes on (captured in
    /// `__agent-begin`); `None` for a top-level (non-task) run. When that task is
    /// CANCELLED its bytecode never resumes, so `__agent-finish` never fires —
    /// the task-reaped sweep (`reap_cancelled_agent_runs`) matches on this id to
    /// reclaim the entry (and end its span) instead of leaking it until
    /// `reset_runtime_state`.
    owning_task_id: Option<u64>,
}

impl Drop for AgentLoopState {
    fn drop(&mut self) {
        // Normal path (`__agent-finish`, or `reset_runtime_state` during eval): the otel
        // thread-locals are live, so let the span guard pop+end and the scope guard
        // restore — dropping the span BEFORE the scope (reverse of begin's install order).
        if sema_otel::tls_alive() {
            drop(self.agent_span.take());
            drop(self.conv_guard.take());
        } else {
            // Thread teardown of a leaked (cancelled) run: the otel thread-locals are
            // already destroyed. Forget the guards rather than let their `Drop` touch
            // dead TLS and abort the process. The span never flushes, which is
            // acceptable for a cancelled run at process exit.
            std::mem::forget(self.agent_span.take());
            std::mem::forget(self.conv_guard.take());
        }
    }
}

thread_local! {
    /// Live non-blocking agent runs, keyed by the integer token handed to Sema.
    static AGENT_RUNS: RefCell<std::collections::HashMap<u64, AgentLoopState>> =
        RefCell::new(std::collections::HashMap::new());
    static AGENT_RUN_NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

/// Clear any live agent-loop state (called from `reset_runtime_state`). Dropping the
/// entries ends any still-open agent spans; benign when otel is disabled.
fn clear_agent_runs() {
    AGENT_RUNS.with(|r| r.borrow_mut().clear());
    AGENT_RUN_NEXT_ID.with(|c| c.set(1));
}

/// Test instrumentation: the number of live entries in the non-blocking agent-run
/// slab. A settled scheduler must leave this at 0 — normal exit and Sema errors go
/// through `__agent-finish`, and a cancelled task's entries are reclaimed by
/// [`reap_cancelled_agent_runs`].
pub fn agent_runs_len() -> usize {
    AGENT_RUNS.with(|r| r.borrow().len())
}

/// Test instrumentation: number of live stream-run slab entries.
pub fn stream_runs_len() -> usize {
    STREAM_RUNS.with(|r| r.borrow().len())
}

/// Task-reaped sweep (registered via `sema_core::set_task_reaped_callback`): when
/// the scheduler cancels a task it will never resume, remove every slab entry that
/// task owns. `__agent-finish` cannot run for a cancelled task (its bytecode is
/// gone), so this is the entry's only reclamation point before
/// `reset_runtime_state`. Runs on the VM thread with OTel TLS alive, but with the
/// CANCELLER's otel context installed — not the dead task's — so the span/scope
/// guards must not touch the installed stack/ids:
/// - the agent span ends via `end_unstacked` (its pushed context lives on the dead
///   task's saved span stack; a popping end would mis-pop the canceller's stack);
/// - the conversation guard is `defuse`d (restoring its saved prev ids would
///   clobber the canceller's).
///
/// Idempotent by absence in both directions: after `__agent-finish` removed the
/// entry this sweep finds nothing, and a late finish after this sweep is the
/// existing idempotent no-op. Entries with `owning_task_id: None` are untouched.
fn reap_cancelled_agent_runs(task_id: u64) {
    let reaped: Vec<AgentLoopState> = AGENT_RUNS.with(|r| {
        let mut slab = r.borrow_mut();
        let tokens: Vec<u64> = slab
            .iter()
            .filter(|(_, st)| st.owning_task_id == Some(task_id))
            .map(|(k, _)| *k)
            .collect();
        tokens.into_iter().filter_map(|t| slab.remove(&t)).collect()
    });
    for mut st in reaped {
        if let Some(span) = st.agent_span.take() {
            span.record_error("cancelled", "agent run cancelled");
            span.end_unstacked();
        }
        if let Some(guard) = st.conv_guard.take() {
            guard.defuse();
        }
        // The rest (messages, tool Values, closures) drops here; `Drop` sees both
        // guards already taken.
    }
    // The stream-run slab is owned by the same tasks (an :on-text agent round or a
    // standalone `llm/stream`) and leaks the same way on cancel — including the
    // DETACHED chat span each entry holds. The seam is single-slot, so this one
    // callback sweeps both slabs. Detached spans end without touching the
    // installed (canceller's) span stack, so a plain end is safe here.
    let stream_reaped: Vec<StreamRunState> = STREAM_RUNS.with(|r| {
        let mut slab = r.borrow_mut();
        let tokens: Vec<u64> = slab
            .iter()
            .filter(|(_, st)| st.owning_task_id == Some(task_id))
            .map(|(k, _)| *k)
            .collect();
        tokens.into_iter().filter_map(|t| slab.remove(&t)).collect()
    });
    for mut st in stream_reaped {
        if let Some(span) = st.span.take() {
            span.record_error("cancelled", "stream cancelled");
        }
        // The wire worker (if still running) streams into a dead channel and
        // releases its admission permit when the provider stream ends —
        // documented best-effort for the sync stream stage.
    }
}

/// Extract the integer handle token from a `__agent-*` native's args.
fn agent_token_arg(args: &[Value], who: &str) -> Result<u64, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity(who, "1", args.len()));
    }
    args[0]
        .as_int()
        .filter(|n| *n >= 0)
        .map(|n| n as u64)
        .ok_or_else(|| SemaError::type_error("agent-run-handle", args[0].type_name()))
}

/// `__agent-begin(agent, input, opts-or-absent) → token-int`. Ports `__agent-run-blocking`'s
/// setup: session/memory seed, conversation-id resolution, message assembly, tool
/// schemas, system, telemetry, and the attached agent span; stores it in the slab.
fn agent_begin(args: &[Value]) -> Result<Value, SemaError> {
    if args.len() < 2 || args.len() > 3 {
        return Err(SemaError::arity("agent/run", "2-3", args.len()));
    }
    let agent = args[0]
        .as_agent_rc()
        .ok_or_else(|| SemaError::type_error("agent", args[0].type_name()))?;
    let user_msg = args[1]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| args[1].to_string());

    let opts = args.get(2).and_then(|v| v.as_map_rc());
    let has_opts = opts.is_some();

    let on_tool_call = opts
        .as_ref()
        .and_then(|o| o.get(&Value::keyword("on-tool-call")).cloned());
    let on_text = opts
        .as_ref()
        .and_then(|o| o.get(&Value::keyword("on-text")).cloned());
    let reasoning_effort = opts
        .as_ref()
        .and_then(|o| get_opt_effort(o, "reasoning-effort"));

    // :session — seed history + conversation-id from a prior Conversation.
    let (session_messages, session_conv_id): (Vec<ChatMessage>, Option<String>) =
        if let Some(ref o) = opts {
            if let Some(sess_val) = o.get(&Value::keyword("session")) {
                if let Some(conv_rc) = sess_val.as_conversation_rc() {
                    let msgs: Vec<ChatMessage> = conv_rc
                        .messages
                        .iter()
                        .map(|m| ChatMessage::new(m.role.to_string(), m.content.clone()))
                        .collect();
                    let cid = conv_rc.metadata.get("conversation-id").cloned();
                    (msgs, cid)
                } else {
                    (Vec::new(), None)
                }
            } else {
                (Vec::new(), None)
            }
        } else {
            (Vec::new(), None)
        };

    // :memory — seed from the memory working set.
    let memory_handle: Option<Value> = opts
        .as_ref()
        .and_then(|o| o.get(&Value::keyword("memory")).cloned());
    let memory_seed: Vec<ChatMessage> = if let Some(ref h) = memory_handle {
        MEMORY_CALLBACKS.with(|c| {
            if let Some(ref cbs) = *c.borrow() {
                (cbs.get_working)(h).unwrap_or_default()
            } else {
                Vec::new()
            }
        })
    } else {
        Vec::new()
    };

    let output_conv_id: String = session_conv_id
        .clone()
        .or_else(|| {
            opts.as_ref()
                .and_then(|o| o.get(&Value::keyword("conversation-id")))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        })
        .unwrap_or_else(sema_otel::new_conversation_id);

    let conv_scope = ConvScope {
        conversation: Some(output_conv_id.clone()),
        session: opts
            .as_ref()
            .and_then(|o| o.get(&Value::keyword("session-id")))
            .and_then(|v| v.as_str().map(|s| s.to_string())),
        user: opts
            .as_ref()
            .and_then(|o| o.get(&Value::keyword("user-id")))
            .and_then(|v| v.as_str().map(|s| s.to_string())),
    };

    // Build messages: memory working set + session history + :messages history + new user.
    let mut messages: Vec<ChatMessage> = memory_seed;
    messages.extend(session_messages);
    if let Some(ref o) = opts {
        if let Some(history) = o.get(&Value::keyword("messages")) {
            let extra = sema_list_to_chat_messages(history)?;
            messages.extend(extra);
        }
    }
    let pre_user_count = messages.len();
    messages.push(ChatMessage::new("user", user_msg));

    let tool_schemas = build_tool_schemas(&agent.tools)?;
    let system = if agent.system.is_empty() {
        None
    } else {
        Some(agent.system.clone())
    };

    let first_input = messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.to_text())
        .unwrap_or_default();

    // Open conversation scope FIRST so the agent span carries the same ids, then start
    // the attached agent span (pushed onto the thread-local span stack; the per-task
    // otel swap preserves it across every park). Both guards live in the slab and are
    // dropped (balanced pop+end) in `__agent-finish` / `Drop`.
    let conv_guard = Some(sema_otel::set_conversation_scope(
        &output_conv_id,
        conv_scope.session.as_deref(),
        conv_scope.user.as_deref(),
    ));
    let agent_span = sema_otel::agent_span(Some(&agent.name));
    // User :tags / :metadata attached directly to the agent span (a `CallTelemetry`
    // guard cannot be held across the loop's yields; the async path attaches to the
    // agent root rather than threading CALL_TAGS through every round).
    if let Some(o) = opts.as_ref() {
        let tags = get_opt_string_list(o, "tags");
        let meta = get_opt_str_map(o, "metadata");
        if !tags.is_empty() {
            agent_span.set_tags(&tags);
        }
        if !meta.is_empty() {
            agent_span.set_metadata(&meta);
        }
    }

    let state = AgentLoopState {
        messages,
        tools: agent.tools.clone(),
        tool_schemas,
        model: agent.model.clone(),
        max_tokens: Some(4096),
        temperature: None,
        system,
        reasoning_effort,
        on_tool_call,
        on_text,
        round: 0,
        max_rounds: agent.max_turns,
        consecutive_errors: 0,
        pending_tool_calls: Vec::new(),
        last_content: String::new(),
        first_input,
        done: false,
        abort_error: None,
        final_pushed: false,
        output_conv_id,
        has_opts,
        memory_handle,
        pre_user_count,
        agent_model: agent.model.clone(),
        agent_span: Some(agent_span),
        conv_guard,
        owning_task_id: sema_core::current_task_id(),
    };

    let token = AGENT_RUN_NEXT_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    });
    AGENT_RUNS.with(|r| r.borrow_mut().insert(token, state));
    Ok(Value::int(token as i64))
}

/// Apply one provider round's response to the loop state and return the driver's
/// `{:done bool :has-tools bool}` map. Runs on the VM thread (either the poller after
/// an async round, or inline for the synchronous fallback). Short-borrows the slab.
fn agent_apply_step_response(token: u64, resp: ChatResponse) -> Result<Value, SemaError> {
    AGENT_RUNS.with(|r| {
        let mut slab = r.borrow_mut();
        let st = slab
            .get_mut(&token)
            .ok_or_else(|| SemaError::Llm("agent-run handle not found".to_string()))?;
        st.last_content = resp.content.clone();
        let has_tools = !resp.tool_calls.is_empty();
        if has_tools {
            // Echo the assistant turn carrying tool_calls BEFORE the tool results, so
            // every provider can correlate them (OpenAI rejects orphan tool results).
            st.messages.push(ChatMessage::assistant_with_tool_calls(
                resp.content.clone(),
                resp.tool_calls.clone(),
            ));
            st.pending_tool_calls = resp.tool_calls;
            st.round += 1;
            // Round cap: mark done, but `:has-tools` stays true so the driver still runs
            // this round's tools (`__agent-exec-tools`) before finishing — matching
            // `run_tool_loop`, which executes the final round's tools and so leaves a
            // valid `assistant(tool_calls) → tool_result` history rather than a dangling
            // tool-call turn that a follow-up run would feed back and providers reject.
            if st.round >= st.max_rounds {
                st.done = true;
            }
        } else {
            // No tool calls → final turn; `__agent-finish` appends the plain assistant.
            st.done = true;
        }
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("done"), Value::bool(st.done));
        map.insert(Value::keyword("has-tools"), Value::bool(has_tools));
        Ok(Value::map(map))
    })
}

/// `__agent-step(token) → {:done bool :has-tools bool}`. One provider round: in async
/// context it offloads + yields `AwaitIo` (the map is produced by the poller via the
/// finalize closure and becomes the resolved value of the yield); otherwise it runs
/// `do_complete` synchronously. If the loop is already done (round cap or consec-error
/// abort set by `__agent-exec-tools`), returns immediately without a provider call.
fn agent_step(ctx: &EvalContext, token: u64) -> Result<Value, SemaError> {
    // A resumed AwaitIo yield lands here NOT re-invoked (the scheduler resumes the
    // bytecode after the CALL); but drain any stray resume value defensively, as the
    // other yielding natives do.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    // Short-borrow: bail out if the loop is already done (round cap / consec-error
    // abort), else build the request + snapshot on_text; then drop the borrow.
    enum StepPrep {
        Done,
        Run(Box<ChatRequest>, Option<Value>),
    }
    let prep = AGENT_RUNS.with(|r| {
        let slab = r.borrow();
        let st = slab
            .get(&token)
            .ok_or_else(|| SemaError::Llm("agent-run handle not found".to_string()))?;
        if st.done {
            return Ok(StepPrep::Done);
        }
        let mut request = ChatRequest::new(st.model.clone(), st.messages.clone());
        request.max_tokens = st.max_tokens.or(Some(4096));
        request.temperature = st.temperature;
        request.system = st.system.clone();
        request.reasoning_effort = st.reasoning_effort.clone();
        request.tools = st.tool_schemas.clone();
        Ok::<_, SemaError>(StepPrep::Run(Box::new(request), st.on_text.clone()))
    })?;

    let (request, on_text) = match prep {
        StepPrep::Done => {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("done"), Value::bool(true));
            map.insert(Value::keyword("has-tools"), Value::bool(false));
            return Ok(Value::map(map));
        }
        StepPrep::Run(req, on_text) => (*req, on_text),
    };

    // Async context: a plain round offloads + yields; a streaming (`:on-text`)
    // round opens a non-blocking stream run and hands the driver
    // `{:stream tok :on-text cb}` — the prelude drives `__stream-drive` in TASK
    // context (so the callback may itself yield, and siblings interleave between
    // delta batches), then applies the assembled response via
    // `__agent-stream-apply`, feeding `agent_apply_step_response` unchanged.
    #[cfg(not(target_arch = "wasm32"))]
    if sema_core::in_async_context() {
        if let Some(cb) = on_text {
            // Mirror `do_complete_streaming`'s scope/span setup, detached (the
            // span is finalized by the stream poller after the last park).
            let _conv = (sema_otel::current_conversation_id().is_none()).then(|| {
                sema_otel::set_conversation_scope(&sema_otel::new_conversation_id(), None, None)
            });
            let span = sema_otel::llm_span_detached("chat");
            span.set_request(
                request.temperature,
                request.max_tokens,
                &request.stop_sequences,
                request.reasoning_effort.as_deref(),
            );
            span.set_output_type(request.json_mode);
            let stream_token = stream_run_begin(request, span)?;
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("stream"), stream_token);
            map.insert(Value::keyword("on-text"), cb);
            return Ok(Value::map(map));
        }
        return do_complete_async_yield(
            request,
            Box::new(move |resp| agent_apply_step_response(token, resp)),
        );
    }

    // Synchronous round: an `:on-text` streaming round drives the SSE stream inline on
    // the VM thread; a plain round in non-async context is the ordinary blocking
    // completion. Either way the usage is accounted once and the state updated inline.
    let response = match on_text.as_ref() {
        Some(cb) => do_complete_streaming(ctx, request, cb)?,
        None => do_complete(request)?,
    };
    track_usage(&response.usage)?;
    agent_apply_step_response(token, response)
}

/// `__agent-exec-tools(token) → nil`. Runs the pending tool calls in ordinary async
/// task context (so yielding/async tools suspend correctly), pushing correlated
/// tool-result messages. Never holds the slab borrow across a callback / tool call.
fn agent_exec_tools(ctx: &EvalContext, token: u64) -> Result<Value, SemaError> {
    // Short-borrow: copy out the pending calls + tool set + callback, then drop.
    let (pending, tools, on_tool_call): (Vec<ToolCall>, Vec<Value>, Option<Value>) = AGENT_RUNS
        .with(|r| {
            let mut slab = r.borrow_mut();
            let st = slab
                .get_mut(&token)
                .ok_or_else(|| SemaError::Llm("agent-run handle not found".to_string()))?;
            let pending = std::mem::take(&mut st.pending_tool_calls);
            Ok::<_, SemaError>((pending, st.tools.clone(), st.on_tool_call.clone()))
        })?;

    for tc in &pending {
        let args_value = sema_core::json_to_value(&tc.arguments);

        if let Some(callback) = on_tool_call.as_ref() {
            let mut event_map = BTreeMap::new();
            event_map.insert(Value::keyword("event"), Value::string("start"));
            event_map.insert(Value::keyword("tool"), Value::string(&tc.name));
            event_map.insert(Value::keyword("args"), args_value.clone());
            let _ = sema_core::call_callback(ctx, callback, &[Value::map(event_map)]);
        }

        let start_time = std::time::Instant::now();
        let tool_desc = tools.iter().find_map(|t| {
            let td = t.as_tool_def_rc()?;
            (td.name == tc.name).then(|| td.description.clone())
        });
        let tspan = sema_otel::tool_span(&tc.name, &tc.id, tool_desc.as_deref());
        let (result, is_error) = match execute_tool_call(ctx, &tools, &tc.name, &tc.arguments) {
            Ok(r) => (r, false),
            Err(e) => (format!("Error: {e}"), true),
        };
        if is_error {
            tspan.record_error("tool_error", &result);
        }
        if sema_otel::content_capture_enabled() {
            let args_json = serde_json::to_string(&tc.arguments).unwrap_or_default();
            tspan.set_tool_io(&args_json, &result);
        }
        drop(tspan);
        let duration_ms = start_time.elapsed().as_millis() as i64;

        if let Some(callback) = on_tool_call.as_ref() {
            let mut event_map = BTreeMap::new();
            event_map.insert(Value::keyword("event"), Value::string("end"));
            event_map.insert(Value::keyword("tool"), Value::string(&tc.name));
            event_map.insert(Value::keyword("args"), args_value);
            let result_preview = if result.len() > 200 {
                format!("{}...", sema_core::truncate_chars(&result, 200))
            } else {
                result.clone()
            };
            event_map.insert(Value::keyword("result"), Value::string(&result_preview));
            event_map.insert(Value::keyword("error"), Value::bool(is_error));
            event_map.insert(Value::keyword("duration-ms"), Value::int(duration_ms));
            let _ = sema_core::call_callback(ctx, callback, &[Value::map(event_map)]);
        }

        // Re-borrow to push the correlated result + update the error counter.
        AGENT_RUNS.with(|r| {
            let mut slab = r.borrow_mut();
            let st = match slab.get_mut(&token) {
                Some(st) => st,
                None => return,
            };
            st.messages.push(ChatMessage::tool_result(
                tc.id.clone(),
                tc.name.clone(),
                result,
            ));
            if is_error {
                st.consecutive_errors += 1;
                if st.consecutive_errors >= MAX_CONSECUTIVE_TOOL_ERRORS {
                    // Stop the loop; `__agent-finish` raises the abort so the caller
                    // sees the same failure the blocking path returns via `?`.
                    st.done = true;
                    st.abort_error = Some(format!(
                        "aborting agent run after {} consecutive tool errors",
                        st.consecutive_errors
                    ));
                }
            } else {
                st.consecutive_errors = 0;
            }
        });
    }

    Ok(Value::nil())
}

/// `__agent-finish(token) → result`. Idempotent: appends the final assistant turn,
/// records trace I/O, ends the agent span, writes back to memory, and builds the
/// return value (`{:response :messages :session}` map with opts, else the string).
fn agent_finish(token: u64) -> Result<Value, SemaError> {
    // Take the state OUT of the slab so the span/scope guards drop (balanced pop+end,
    // agent-task otel installed) once we're done building the result.
    let mut st = match AGENT_RUNS.with(|r| r.borrow_mut().remove(&token)) {
        Some(st) => st,
        // Already finished (idempotent) — the driver's normal exit and the Sema catch
        // may both call finish.
        None => return Ok(Value::nil()),
    };

    // Append the final assistant message (mirrors run_tool_loop's terminal push).
    if !st.final_pushed && !st.last_content.is_empty() {
        st.messages
            .push(ChatMessage::new("assistant", st.last_content.clone()));
        st.final_pushed = true;
    }
    if let Some(span) = st.agent_span.as_ref() {
        span.set_trace_io(&st.first_input, &st.last_content);
    }

    // Memory writeback: append new turns (from pre_user_count) into the memory thread.
    if let Some(ref h) = st.memory_handle {
        let new_turns = if st.messages.len() > st.pre_user_count {
            &st.messages[st.pre_user_count..]
        } else {
            &[]
        };
        MEMORY_CALLBACKS.with(|c| {
            if let Some(ref cbs) = *c.borrow() {
                let _ = (cbs.append_back)(h, new_turns);
            }
        });
    }

    // A consecutive-tool-error abort surfaces as an error (matching the blocking path).
    if let Some(msg) = st.abort_error.take() {
        if let Some(span) = st.agent_span.as_ref() {
            span.record_error("tool_error", &msg);
        }
        // `st` drops here → agent span ends, conv scope restored.
        return Err(SemaError::Llm(msg));
    }

    let result = st.last_content.clone();
    if st.has_opts {
        let mut meta = std::collections::BTreeMap::new();
        meta.insert("conversation-id".to_string(), st.output_conv_id.clone());
        let session_conv = Conversation {
            messages: st
                .messages
                .iter()
                .map(|m| Message {
                    role: match m.role.as_str() {
                        "assistant" => Role::Assistant,
                        _ => Role::User,
                    },
                    content: m.content.to_text(),
                    images: Vec::new(),
                })
                .collect(),
            model: st.agent_model.clone(),
            metadata: meta,
        };
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("response"), Value::string(&result));
        map.insert(
            Value::keyword("messages"),
            chat_messages_to_sema_list(&st.messages),
        );
        map.insert(Value::keyword("session"), Value::conversation(session_conv));
        Ok(Value::map(map))
    } else {
        Ok(Value::string(&result))
    }
    // `st` drops at the end of scope → agent span ends (balanced), conv scope restored.
}

// ── Non-blocking streaming (`llm/stream` + agent `:on-text` rounds) ──────────
//
// The streaming sibling of the `__agent-*` loop above, same ADR #68 shape: a
// native cannot loop-yield (a yielded `AwaitIo` is not re-invoked, and a Sema
// callback cannot run inside a poller), so the per-delta loop lives in bytecode
// (`__stream-drive` in the prelude) over three natives — `__stream-begin` /
// `__stream-next` / `__stream-finish` — coordinated by a slab entry that owns
// the wire channel and the finalize context. The wire side (the provider's
// synchronous SSE drive) runs on the I/O pool, sending each delta over an mpsc
// channel; only `String`s and the final `ChatResponse` cross the thread
// boundary, never a Sema `Value`.

/// One event on a stream run's wire channel.
enum StreamEvent {
    /// A text delta, in arrival order.
    Delta(String),
    /// Terminal event: the assembled response (or the stream failure) plus the
    /// registry name of the provider that served (or last attempted) it.
    Done(Box<StreamDone>),
}

struct StreamDone {
    result: Result<ChatResponse, LlmError>,
    provider: String,
}

/// Per-run state for a non-blocking stream, keyed by an integer token in the
/// thread-local `STREAM_RUNS` slab (its own slab — agent-loop state is not
/// touched). Owns the wire receiver and everything the finalize needs on the VM
/// thread after the last park.
struct StreamRunState {
    /// Wire-side receiver (`None` for pre-filled runs: cassette replay).
    rx: Option<std::sync::mpsc::Receiver<StreamEvent>>,
    /// Pre-filled events, drained before `rx`.
    buffered: std::collections::VecDeque<StreamEvent>,
    /// Detached chat span (parent captured at begin), finalized when `Done` lands.
    span: Option<sema_otel::LlmSpan>,
    /// This leaf's usage-accumulator frame, captured at begin (same reasoning as
    /// `do_complete_async_yield`'s `usage_accum_slot`).
    usage_accum_slot: Option<Rc<RefCell<LeafUsage>>>,
    /// The dispatch-time budget frame `Rc` (ASYNC-1), re-installed around the
    /// finalize's `track_usage`.
    budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
    /// Set when a cassette is recording; the entry is written at finalize with
    /// the collected chunk boundaries.
    cassette_record_key: Option<String>,
    /// Every delta drained so far (cassette recording preserves boundaries).
    collected: Vec<String>,
    first_token_seen: bool,
    /// The assembled response, set once `Done(Ok)` has been finalized.
    response: Option<ChatResponse>,
    done: bool,
    /// The scheduler task that opened this run (None outside a task). The
    /// task-reaped sweep reclaims entries by this id when their task is
    /// cancelled — `__stream-finish` cannot run for a cancelled task.
    owning_task_id: Option<u64>,
    /// A failure that arrived in a batch that still carried deltas: stored so the
    /// driver delivers those deltas to the callback first, then raised (and the
    /// entry dropped) on the next `__stream-next`/`__stream-finish`.
    pending_error: Option<String>,
}

impl Drop for StreamRunState {
    fn drop(&mut self) {
        // Normal path (finalize already took the span, or `reset_runtime_state`
        // during eval): let the detached span end with the otel thread-locals
        // alive. Thread teardown of a leaked (cancelled) run: forget the span
        // rather than let its `Drop` touch dead TLS and abort the process
        // (mirrors `AgentLoopState`).
        if !sema_otel::tls_alive() {
            std::mem::forget(self.span.take());
        }
    }
}

thread_local! {
    /// Live non-blocking stream runs, keyed by the integer token handed to Sema.
    static STREAM_RUNS: RefCell<std::collections::HashMap<u64, StreamRunState>> =
        RefCell::new(std::collections::HashMap::new());
    static STREAM_RUN_NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

/// Clear any live stream-run state (called from `reset_runtime_state`). A task
/// cancelled while parked in `__stream-next` abandons its entry here (the wire
/// worker keeps streaming into the never-drained channel and is discarded —
/// best-effort, like completion offloads), so this is also the leak backstop.
fn clear_stream_runs() {
    STREAM_RUNS.with(|r| r.borrow_mut().clear());
    STREAM_RUN_NEXT_ID.with(|c| c.set(1));
}

/// Extract the integer token from a `__stream-*` native's arg.
fn stream_token_arg(v: &Value) -> Result<u64, SemaError> {
    v.as_int()
        .filter(|n| *n >= 0)
        .map(|n| n as u64)
        .ok_or_else(|| SemaError::type_error("stream-run-handle", v.type_name()))
}

/// Walk the resolved provider chain, opening the stream against each provider in
/// turn and emitting `StreamEvent`s. Fail-over happens ONLY before the first
/// delta; once a chunk is out a mid-stream error surfaces (failing over would
/// re-emit the already-delivered partial — same policy as
/// `stream_with_dispatch`). Runs on a pool worker, so it touches NO
/// thread-locals; always ends with exactly one `Done`.
fn stream_wire_walk(
    chain: &[ResolvedProvider],
    request: &ChatRequest,
    emit: &mut dyn FnMut(StreamEvent),
) {
    let mut last: Option<(LlmError, String)> = None;
    for entry in chain {
        let mut req = request.clone();
        if let Some(m) = &entry.model {
            req.model = m.clone();
        } else if req.model.is_empty() {
            req.model = entry.provider.default_model().to_string();
        }
        let mut emitted = false;
        let result = {
            let mut cb = |c: &str| -> Result<(), LlmError> {
                emitted = true;
                emit(StreamEvent::Delta(c.to_string()));
                Ok(())
            };
            entry.provider.stream_complete(req, &mut cb)
        };
        match result {
            Ok(resp) => {
                emit(StreamEvent::Done(Box::new(StreamDone {
                    result: Ok(resp),
                    provider: entry.name.clone(),
                })));
                return;
            }
            Err(e) if emitted => {
                // Mid-stream failure: surface; do NOT fail over (would duplicate).
                emit(StreamEvent::Done(Box::new(StreamDone {
                    result: Err(e),
                    provider: entry.name.clone(),
                })));
                return;
            }
            Err(e) => {
                eprintln!(
                    "Provider '{}' failed to open stream: {e}, trying next...",
                    entry.name
                );
                last = Some((e, entry.name.clone()));
            }
        }
    }
    let (e, name) = last.unwrap_or_else(|| {
        (
            LlmError::Config("all providers failed".to_string()),
            String::new(),
        )
    });
    emit(StreamEvent::Done(Box::new(StreamDone {
        result: Err(e),
        provider: name,
    })));
}

/// Resolve the active fallback chain (or the default provider) into owned `Arc`
/// clones on the VM thread, so the offloaded wire walk touches no thread-locals
/// (mirrors the resolution inside `do_complete_async_yield`).
fn resolve_stream_chain() -> Result<Vec<ResolvedProvider>, SemaError> {
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let fallback = FALLBACK_CHAIN.with(|c| c.borrow().clone());
        match fallback {
            Some(entries) if !entries.is_empty() => entries
                .iter()
                .map(|e| {
                    reg.get(&e.provider)
                        .map(|p| ResolvedProvider {
                            provider: p,
                            name: e.provider.clone(),
                            model: e.model.clone(),
                        })
                        .ok_or_else(|| {
                            SemaError::Llm(format!("fallback provider '{}' not found", e.provider))
                        })
                })
                .collect::<Result<Vec<_>, _>>(),
            _ => {
                let p = reg.default_provider().ok_or_else(|| {
                    SemaError::Llm(
                        "no LLM provider configured. Use (llm/configure :anthropic \
                         {:api-key ...}) first"
                            .to_string(),
                    )
                })?;
                let name = p.name().to_string();
                Ok(vec![ResolvedProvider {
                    provider: p,
                    name,
                    model: None,
                }])
            }
        }
    })
}

/// Start a non-blocking stream run: budget pre-gate + rate limit, cassette
/// decision on the VM thread (replay pre-fills the run — drained without
/// parking; recording captures the key for finalize), chain resolution into
/// `Arc` clones, then the wire walk offloaded onto the I/O pool with
/// `notify_io_complete` after every send so the parked scheduler wakes per
/// delta. `span` is the caller's DETACHED chat span (attributes already set);
/// it is finalized when `Done` lands. Returns the slab token.
fn stream_run_begin(request: ChatRequest, span: sema_otel::LlmSpan) -> Result<Value, SemaError> {
    stream_budget_pregate()?;
    enforce_rate_limit();

    // Keyed by the request as-is (no default-model resolution), matching the
    // synchronous `stream_with_cassette` so record/replay agree across paths.
    let cassette_decision = CASSETTE.with(|c| {
        c.borrow().as_ref().map(|cass| {
            let key = compute_cache_key(&request);
            (key.clone(), cass.decide(&key))
        })
    });
    let mut buffered = std::collections::VecDeque::new();
    let mut cassette_record_key = None;
    let mut prefilled = false;
    match cassette_decision {
        Some((_, crate::cassette::Decision::Replay(entry))) => {
            for ch in &entry.chunks {
                buffered.push_back(StreamEvent::Delta(ch.clone()));
            }
            buffered.push_back(StreamEvent::Done(Box::new(StreamDone {
                result: Ok(entry.to_response()),
                provider: "cassette".to_string(),
            })));
            prefilled = true;
        }
        Some((k, crate::cassette::Decision::Miss(_))) => return Err(cassette_miss_error(&k)),
        Some((k, crate::cassette::Decision::Record)) => cassette_record_key = Some(k),
        _ => {}
    }

    let rx = if prefilled {
        None
    } else {
        let chain = resolve_stream_chain()?;
        let (tx, rx) = std::sync::mpsc::channel::<StreamEvent>();
        #[cfg(not(target_arch = "wasm32"))]
        {
            sema_io::io_spawn_blocking(move || {
                let mut emit = |ev: StreamEvent| {
                    let _ = tx.send(ev);
                    sema_core::notify_io_complete();
                };
                stream_wire_walk(&chain, &request, &mut emit);
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            // No I/O pool on wasm: run the walk inline (blocking) — deltas are
            // delivered after the stream completes, in order, exactly once.
            let mut emit = |ev: StreamEvent| {
                let _ = tx.send(ev);
            };
            stream_wire_walk(&chain, &request, &mut emit);
        }
        Some(rx)
    };

    let state = StreamRunState {
        rx,
        buffered,
        span: Some(span),
        usage_accum_slot: current_usage_accum(),
        budget_slot: active_budget(),
        cassette_record_key,
        collected: Vec::new(),
        first_token_seen: false,
        response: None,
        done: false,
        owning_task_id: sema_core::current_task_id(),
        pending_error: None,
    };
    let token = STREAM_RUN_NEXT_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    });
    STREAM_RUNS.with(|r| r.borrow_mut().insert(token, state));
    Ok(Value::int(token as i64))
}

/// Post-stream work on the VM thread when `Done` lands — the streaming analogue
/// of `do_complete_async_yield`'s poller finalize: span finalize,
/// serving-provider stamp, cassette record, per-leaf usage fold +
/// budget-installed `track_usage` (exactly once per streamed completion).
/// Returns the response, or the error message to surface.
fn stream_finalize(
    done: StreamDone,
    span: Option<sema_otel::LlmSpan>,
    usage_accum_slot: Option<Rc<RefCell<LeafUsage>>>,
    budget_slot: Option<Rc<RefCell<BudgetFrame>>>,
    cassette_record_key: Option<String>,
    collected: &[String],
) -> Result<ChatResponse, String> {
    match done.result {
        Ok(resp) => {
            if let Some(span) = span {
                span.set_dispatch(&done.provider, &resp.model);
                span.set_response(&response_facts(&done.provider, &resp));
                // span drops here → ends the span.
            }
            // A cassette replay made no provider call — leave the serving stamp
            // alone (matches the sync no-chain path's canonical pricing).
            if done.provider != "cassette" && !done.provider.is_empty() {
                set_serving_provider(&done.provider);
            }
            if let Some(key) = &cassette_record_key {
                CASSETTE.with(|c| {
                    if let Some(cass) = c.borrow_mut().as_mut() {
                        cass.record_entry(crate::cassette::TapeEntry::from_stream(
                            key, collected, &resp,
                        ));
                    }
                });
            }
            // Fold into THIS run's captured accumulator frame, then suppress
            // `track_usage`'s own fold — the poller runs outside the per-task
            // install boundary, so the thread-local may hold a sibling's scope.
            if let Some(slot) = &usage_accum_slot {
                let cost = pricing::calculate_cost_for(&done.provider, &resp.usage);
                accumulate_into(slot, &resp.usage, cost);
            }
            let track_result = {
                let prev_budget = ACTIVE_BUDGET
                    .with(|b| std::mem::replace(&mut *b.borrow_mut(), budget_slot.clone()));
                let r = USAGE_ACCUM_SUPPRESS.with(|s| {
                    s.set(true);
                    let r = track_usage(&resp.usage);
                    s.set(false);
                    r
                });
                ACTIVE_BUDGET.with(|b| *b.borrow_mut() = prev_budget);
                r
            };
            match track_result {
                Ok(()) => Ok(resp),
                Err(e) => Err(e.to_string()),
            }
        }
        Err(e) => {
            if let Some(span) = span {
                span.record_error(llm_error_kind(&e), &e.to_string());
            }
            Err(e.to_string())
        }
    }
}

/// Build the `{:deltas [...] :done bool}` batch map `__stream-next` resolves to.
fn stream_batch_map(deltas: Vec<Value>, done: bool) -> Value {
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("deltas"), Value::list(deltas));
    map.insert(Value::keyword("done"), Value::bool(done));
    Value::map(map)
}

/// Drain every currently-available wire event for `token` into one batch
/// (batching amortizes park/resume over fast token streams), finalizing the run
/// when `Done` arrives. `blocking` waits for the first event (the sync-context
/// fallback and pre-filled runs); the poller path never blocks. `Ok(None)` =
/// nothing available yet (stay parked).
///
/// A failure is NEVER surfaced through the poller (an `IoPoll::Ready(Err)`
/// rejects the whole task — an in-task `try` could not catch it, and which path
/// fired would depend on batch timing): it is always stored as `pending_error`
/// and raised by the NEXT `__stream-next` as an ordinary native error —
/// deterministic, catchable in task context (matching the sync path), and the
/// callback still sees every delta delivered before the failure.
fn stream_poll_batch(token: u64, blocking: bool) -> Result<Option<Value>, SemaError> {
    use std::sync::mpsc::TryRecvError;

    let mut batch: Vec<Value> = Vec::new();
    let mut done_event: Option<Box<StreamDone>> = None;
    let mut closed = false;

    // Short-borrow the slab: drain buffered events + the channel into the batch.
    STREAM_RUNS.with(|r| -> Result<(), SemaError> {
        let mut slab = r.borrow_mut();
        let st = slab
            .get_mut(&token)
            .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
        loop {
            let ev = if let Some(ev) = st.buffered.pop_front() {
                Some(ev)
            } else if let Some(rx) = &st.rx {
                if blocking && batch.is_empty() {
                    match rx.recv() {
                        Ok(ev) => Some(ev),
                        Err(_) => {
                            closed = true;
                            None
                        }
                    }
                } else {
                    match rx.try_recv() {
                        Ok(ev) => Some(ev),
                        Err(TryRecvError::Empty) => None,
                        Err(TryRecvError::Disconnected) => {
                            closed = true;
                            None
                        }
                    }
                }
            } else {
                None
            };
            match ev {
                Some(StreamEvent::Delta(s)) => {
                    if !st.first_token_seen {
                        st.first_token_seen = true;
                        if let Some(span) = st.span.as_ref() {
                            span.mark_first_token();
                        }
                    }
                    batch.push(Value::string(&s));
                    st.collected.push(s);
                }
                Some(StreamEvent::Done(d)) => {
                    done_event = Some(d);
                    break;
                }
                None => break,
            }
        }
        Ok(())
    })?;

    // A closed channel without a Done means the worker died mid-stream.
    if done_event.is_none() && closed {
        done_event = Some(Box::new(StreamDone {
            result: Err(LlmError::Config("stream: io worker dropped".to_string())),
            provider: String::new(),
        }));
    }

    let Some(done) = done_event else {
        return if batch.is_empty() {
            Ok(None)
        } else {
            Ok(Some(stream_batch_map(batch, false)))
        };
    };

    // Done landed: take the finalize context out (short-borrow), run the
    // finalize outside the slab borrow, then write the outcome back.
    let ctx = STREAM_RUNS.with(|r| {
        let mut slab = r.borrow_mut();
        slab.get_mut(&token).map(|st| {
            (
                st.span.take(),
                st.usage_accum_slot.take(),
                st.budget_slot.take(),
                st.cassette_record_key.take(),
                std::mem::take(&mut st.collected),
            )
        })
    });
    let Some((span, usage_slot, budget_slot, record_key, collected)) = ctx else {
        return Err(SemaError::Llm("stream-run handle not found".to_string()));
    };
    match stream_finalize(*done, span, usage_slot, budget_slot, record_key, &collected) {
        Ok(resp) => {
            STREAM_RUNS.with(|r| {
                if let Some(st) = r.borrow_mut().get_mut(&token) {
                    st.response = Some(resp);
                    st.done = true;
                }
            });
            Ok(Some(stream_batch_map(batch, true)))
        }
        Err(msg) => {
            STREAM_RUNS.with(|r| {
                if let Some(st) = r.borrow_mut().get_mut(&token) {
                    st.pending_error = Some(msg);
                }
            });
            Ok(Some(stream_batch_map(batch, false)))
        }
    }
}

/// `__stream-next(token) → {:deltas [str…] :done bool}`. In a scheduler task it
/// parks on `AwaitIo`; the poller drains all currently-available deltas per wake
/// (see `stream_poll_batch`). Pre-filled runs and non-async context drain
/// blockingly without parking.
fn stream_next(token: u64) -> Result<Value, SemaError> {
    // The scheduler resumes the bytecode after the CALL via `replace_stack_top`
    // (this native is not re-invoked); drain a stray resume value defensively.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    enum Pre {
        Err(String),
        Done,
        Run { prefilled: bool },
    }
    let pre = STREAM_RUNS.with(|r| {
        let mut slab = r.borrow_mut();
        let st = slab
            .get_mut(&token)
            .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
        if let Some(msg) = st.pending_error.take() {
            // The deltas that preceded this failure were delivered last batch;
            // the run is over — drop the entry and surface.
            slab.remove(&token);
            return Ok(Pre::Err(msg));
        }
        if st.done {
            return Ok(Pre::Done);
        }
        Ok::<_, SemaError>(Pre::Run {
            prefilled: st.rx.is_none(),
        })
    })?;

    let prefilled = match pre {
        Pre::Err(msg) => return Err(SemaError::Llm(msg)),
        Pre::Done => return Ok(stream_batch_map(Vec::new(), true)),
        Pre::Run { prefilled } => prefilled,
    };

    // Pre-filled runs resolve without parking (nothing to overlap — mirrors the
    // cassette-replay no-yield path of `do_complete_async_yield`); outside a
    // scheduler task fall back to a blocking drain.
    if prefilled || !sema_core::in_async_context() {
        loop {
            if let Some(v) = stream_poll_batch(token, true)? {
                return Ok(v);
            }
        }
    }

    let handle = Rc::new(sema_core::IoHandle::new(move || {
        match stream_poll_batch(token, false) {
            Ok(Some(v)) => sema_core::IoPoll::Ready(Ok(v)),
            Ok(None) => sema_core::IoPoll::Pending,
            Err(e) => sema_core::IoPoll::Ready(Err(e.to_string())),
        }
    }));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
}

/// `__stream-finish(token) → content-string`. Cleans the slab entry and returns
/// the assembled content (usage was already accounted, exactly once, when the
/// poller finalized the `Done`).
fn stream_finish(token: u64) -> Result<Value, SemaError> {
    let mut st = STREAM_RUNS
        .with(|r| r.borrow_mut().remove(&token))
        .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
    if let Some(msg) = st.pending_error.take() {
        return Err(SemaError::Llm(msg));
    }
    let resp = st
        .response
        .take()
        .ok_or_else(|| SemaError::Llm("stream not finished".to_string()))?;
    Ok(Value::string(&resp.content))
}

/// `__agent-stream-apply(agent-token, stream-token) → {:done :has-tools}`. The
/// agent-path terminal for a driven streaming round: pops the stream slab entry
/// and feeds the assembled `ChatResponse` to `agent_apply_step_response`
/// unchanged (tool-call handling identical to a non-streaming round; usage was
/// accounted by the stream poller).
fn agent_stream_apply(agent_token: u64, stream_token: u64) -> Result<Value, SemaError> {
    let mut st = STREAM_RUNS
        .with(|r| r.borrow_mut().remove(&stream_token))
        .ok_or_else(|| SemaError::Llm("stream-run handle not found".to_string()))?;
    if let Some(msg) = st.pending_error.take() {
        return Err(SemaError::Llm(msg));
    }
    let resp = st
        .response
        .take()
        .ok_or_else(|| SemaError::Llm("stream not finished".to_string()))?;
    agent_apply_step_response(agent_token, resp)
}

/// The tool execution loop: send -> check for tool_calls -> execute -> send results -> repeat.
#[allow(clippy::too_many_arguments)]
fn run_tool_loop(
    ctx: &EvalContext,
    initial_messages: Vec<ChatMessage>,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    system: Option<String>,
    reasoning_effort: Option<String>,
    tools: &[Value],
    tool_schemas: &[ToolSchema],
    max_rounds: usize,
    on_tool_call: Option<&Value>,
    on_text: Option<&Value>,
    agent_name: Option<&str>,
    ids: ConvScope,
) -> Result<(String, Vec<ChatMessage>), SemaError> {
    // Open the conversation/session/user scope FIRST so the agent span and every
    // nested chat/tool span carry the same gen_ai.conversation.id (+ session.id /
    // user.id). A caller-supplied id wins; otherwise generate a fresh one.
    let conv = ids
        .conversation
        .clone()
        .unwrap_or_else(sema_otel::new_conversation_id);
    let _conv_scope =
        sema_otel::set_conversation_scope(&conv, ids.session.as_deref(), ids.user.as_deref());
    // INTERNAL agent span over the whole loop; the per-round `chat` spans (from
    // do_complete) and per-tool spans nest under it via the thread-local stack.
    let _agent_span = sema_otel::agent_span(agent_name);
    // User :tags / :metadata for this run, attached to the agent root span.
    apply_call_telemetry_agent(&_agent_span);
    let mut messages = initial_messages;
    // First user input for the trace-level I/O rollup (compat: Langfuse trace panel).
    let first_input = messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.to_text())
        .unwrap_or_default();
    let mut last_content = String::new();
    // Bound runaway error loops: if the model keeps issuing failing tool calls
    // and never recovers, abort rather than burning every round. Reset on any
    // successful tool call.
    const MAX_CONSECUTIVE_TOOL_ERRORS: usize = 5;
    let mut consecutive_errors: usize = 0;

    for _round in 0..max_rounds {
        let mut request = ChatRequest::new(model.clone(), messages.clone());
        request.max_tokens = max_tokens.or(Some(4096));
        request.temperature = temperature;
        request.system = system.clone();
        request.reasoning_effort = reasoning_effort.clone();
        request.tools = tool_schemas.to_vec();

        // Stream the assistant text live when the caller supplied :on-text;
        // otherwise take the plain (cache-eligible) path. Tool-call handling and
        // usage accounting below are identical either way.
        let completion = match on_text {
            Some(cb) => do_complete_streaming(ctx, request, cb),
            None => do_complete(request),
        };
        let response = match completion {
            Ok(r) => r,
            Err(e) => {
                _agent_span.record_error("provider_error", &e.to_string());
                return Err(e);
            }
        };
        if let Err(e) = track_usage(&response.usage) {
            _agent_span.record_error("budget_error", &e.to_string());
            return Err(e);
        }
        last_content = response.content.clone();

        if response.tool_calls.is_empty() {
            // Push final assistant message onto history
            if !last_content.is_empty() {
                messages.push(ChatMessage::new("assistant", last_content.clone()));
            }
            _agent_span.set_trace_io(&first_input, &last_content);
            return Ok((last_content, messages));
        }

        // Echo the assistant turn that invoked the tools, carrying the tool_calls
        // so the provider can correlate the tool results that follow. This MUST be
        // present (even with empty content) — OpenAI-family providers reject a
        // tool result that isn't preceded by the assistant tool_calls it answers.
        messages.push(ChatMessage::assistant_with_tool_calls(
            response.content.clone(),
            response.tool_calls.clone(),
        ));

        // Execute each tool call and add results
        for tc in &response.tool_calls {
            // Build args map for callback
            let args_value = sema_core::json_to_value(&tc.arguments);

            // Fire "start" event
            if let Some(callback) = on_tool_call {
                let mut event_map = BTreeMap::new();
                event_map.insert(Value::keyword("event"), Value::string("start"));
                event_map.insert(Value::keyword("tool"), Value::string(&tc.name));
                event_map.insert(Value::keyword("args"), args_value.clone());
                let _ = sema_core::call_callback(ctx, callback, &[Value::map(event_map)]);
            }

            let start_time = std::time::Instant::now();
            // INTERNAL tool span (self-times over execute_tool_call, the one real
            // latency source). v1.41 requires the tool name in the span name.
            let tool_desc = tools.iter().find_map(|t| {
                let td = t.as_tool_def_rc()?;
                (td.name == tc.name).then(|| td.description.clone())
            });
            let tspan = sema_otel::tool_span(&tc.name, &tc.id, tool_desc.as_deref());
            // A failing or invalid tool call must NOT abort the whole agent run.
            // Capture the error as the tool result and feed it back so the model
            // can self-correct (bounded by MAX_CONSECUTIVE_TOOL_ERRORS / max_rounds).
            let (result, is_error) = match execute_tool_call(ctx, tools, &tc.name, &tc.arguments) {
                Ok(r) => {
                    consecutive_errors = 0;
                    (r, false)
                }
                Err(e) => {
                    consecutive_errors += 1;
                    (format!("Error: {e}"), true)
                }
            };
            if is_error {
                tspan.record_error("tool_error", &result);
            }
            // Tool args + result on the span (content-gated; canonical
            // gen_ai.tool.call.* + compat aliases) — the key agent-debugging datum.
            if sema_otel::content_capture_enabled() {
                let args_json = serde_json::to_string(&tc.arguments).unwrap_or_default();
                tspan.set_tool_io(&args_json, &result);
            }
            drop(tspan);
            let duration_ms = start_time.elapsed().as_millis() as i64;

            // Fire "end" event
            if let Some(callback) = on_tool_call {
                let mut event_map = BTreeMap::new();
                event_map.insert(Value::keyword("event"), Value::string("end"));
                event_map.insert(Value::keyword("tool"), Value::string(&tc.name));
                event_map.insert(Value::keyword("args"), args_value);
                // Truncate result for the callback to avoid huge payloads.
                // Use char-boundary truncation: a byte slice (`&result[..200]`)
                // panics when byte 200 lands inside a multi-byte character.
                let result_preview = if result.len() > 200 {
                    format!("{}...", sema_core::truncate_chars(&result, 200))
                } else {
                    result.clone()
                };
                event_map.insert(Value::keyword("result"), Value::string(&result_preview));
                event_map.insert(Value::keyword("error"), Value::bool(is_error));
                event_map.insert(Value::keyword("duration-ms"), Value::int(duration_ms));
                let _ = sema_core::call_callback(ctx, callback, &[Value::map(event_map)]);
            }

            // Correlated tool result — keyed by the call id and tool name — rather
            // than free-form user text, so every provider can match it to the call.
            messages.push(ChatMessage::tool_result(
                tc.id.clone(),
                tc.name.clone(),
                result,
            ));

            if consecutive_errors >= MAX_CONSECUTIVE_TOOL_ERRORS {
                let msg = format!(
                    "aborting agent run after {consecutive_errors} consecutive tool errors"
                );
                _agent_span.record_error("tool_error", &msg);
                return Err(SemaError::Llm(msg));
            }
        }

        // Agent-turn safe point (CORE-2, plan §5.2 point c): the round's tool
        // handlers just ran arbitrary Sema code (the long-running-agent leak
        // shape — recursive local helpers, channels, promises created per
        // turn), and a long agent run never returns to a top-level safe point
        // until it finishes. Threshold-gated. No pins: sema-llm cannot see the
        // executing VM's env (it depends only on sema-core), and pins are a
        // pure descent-skip optimization — correctness comes from external
        // strong counts. Message history/correlation is untouched.
        sema_core::gc_maybe_collect(&[], sema_core::GcTrigger::AgentTurn);
    }

    // Push final assistant message if we exhausted rounds
    if !last_content.is_empty() {
        messages.push(ChatMessage::new("assistant", last_content.clone()));
    }
    _agent_span.set_trace_io(&first_input, &last_content);
    Ok((last_content, messages))
}

/// Execute a tool call by finding the handler and invoking it.
fn execute_tool_call(
    ctx: &EvalContext,
    tools: &[Value],
    name: &str,
    arguments: &serde_json::Value,
) -> Result<String, SemaError> {
    // Find the tool definition
    let tool_def = tools
        .iter()
        .find_map(|t| {
            let td = t.as_tool_def_rc()?;
            if td.name == name {
                Some(td)
            } else {
                None
            }
        })
        .ok_or_else(|| SemaError::Llm(format!("tool not found: {name}")))?;

    // Validate the model-supplied arguments against the tool's parameter schema
    // before invoking the handler, so a missing/wrong-typed argument is reported
    // back to the model (via the loop's error-recovery path) and it can retry with
    // corrected args — rather than silently calling the handler with bad input.
    // (Reuses the extraction validator; both schema and args use keyword keys.)
    let args_map = sema_core::json_to_value(arguments);
    if let Err(msg) = validate_extraction(&args_map, &tool_def.parameters) {
        return Err(SemaError::Llm(format!(
            "invalid arguments for tool '{name}': {msg}"
        )));
    }

    // Convert JSON arguments to Sema values and call the handler
    let sema_args = json_args_to_sema(&tool_def.parameters, arguments, &tool_def.handler);
    let result = sema_core::call_callback(ctx, &tool_def.handler, &sema_args)?;

    // Convert result to string for sending back to LLM
    if let Some(s) = result.as_str() {
        return Ok(s.to_string());
    }
    if result.as_map_rc().is_some() || result.as_seq().is_some() {
        // JSON-encode complex results
        let json = sema_core::value_to_json_lossy(&result);
        Ok(serde_json::to_string(&json).unwrap_or_else(|_| result.to_string()))
    } else {
        Ok(result.to_string())
    }
}

/// Convert JSON arguments into a list of Sema values based on the parameter schema order.
/// When the handler is a lambda, uses its param names (declaration order) instead of
/// BTreeMap key order (alphabetical), fixing argument ordering mismatches.
fn json_args_to_sema(params: &Value, arguments: &serde_json::Value, handler: &Value) -> Vec<Value> {
    if let serde_json::Value::Object(json_obj) = arguments {
        // Prefer lambda param names (preserves declaration order) over BTreeMap keys
        if let Some(lambda) = handler.as_lambda_rc() {
            return lambda
                .params
                .iter()
                .map(|name| {
                    json_obj
                        .get(&resolve(*name))
                        .map(sema_core::json_to_value)
                        .unwrap_or(Value::nil())
                })
                .collect();
        }
        // Fallback: use param map keys (BTreeMap order — alphabetical)
        if let Some(param_map) = params.as_map_rc() {
            return param_map
                .keys()
                .map(|k| {
                    let key_str = k
                        .as_keyword()
                        .or_else(|| k.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| k.to_string());
                    json_obj
                        .get(&key_str)
                        .map(sema_core::json_to_value)
                        .unwrap_or(Value::nil())
                })
                .collect();
        }
    }
    vec![sema_core::json_to_value(arguments)]
}

/// Detect media type from file magic bytes.
fn detect_media_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(b"%PDF") {
        "application/pdf"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::{intern, Lambda};
    use serde_json::json;

    fn usage(prompt: u32, completion: u32) -> Usage {
        Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            model: "fake-model".into(),
            ..Usage::default()
        }
    }

    #[test]
    fn accumulate_into_sums_tokens_cost_and_calls() {
        let slot = Rc::new(RefCell::new(LeafUsage::default()));
        accumulate_into(&slot, &usage(10, 5), Some(0.001));
        accumulate_into(&slot, &usage(100, 50), Some(0.002));
        let u = slot.borrow();
        assert_eq!(u.input_tokens, 110, "tokens sum across calls");
        assert_eq!(u.output_tokens, 55);
        assert!((u.cost_usd.unwrap() - 0.003).abs() < 1e-9, "cost sums");
        assert_eq!(u.calls, 2);
        assert_eq!(u.model, "fake-model");
    }

    #[test]
    fn accumulate_into_skips_cache_hit_zero_usage() {
        // A cache hit is all-zero + unpriced — it must NOT count as a call (no phantom
        // zero Budget event downstream), per the cache-hit-zero-usage invariant.
        let slot = Rc::new(RefCell::new(LeafUsage::default()));
        accumulate_into(&slot, &Usage::default(), None);
        let u = slot.borrow();
        assert_eq!(u.calls, 0, "cache hit doesn't bump calls");
        assert_eq!(u.input_tokens, 0);
        assert!(u.cost_usd.is_none());
    }

    #[test]
    fn accumulate_into_unpriced_call_counts_but_leaves_cost_none() {
        // An unpriced (no pricing-table entry) but token-bearing call IS a call; cost stays
        // genuinely absent rather than $0, then a later priced call seeds the running sum.
        let slot = Rc::new(RefCell::new(LeafUsage::default()));
        accumulate_into(&slot, &usage(7, 3), None);
        assert_eq!(slot.borrow().calls, 1);
        assert!(
            slot.borrow().cost_usd.is_none(),
            "unpriced ⇒ cost still None"
        );
        accumulate_into(&slot, &usage(1, 1), Some(0.005));
        assert!((slot.borrow().cost_usd.unwrap() - 0.005).abs() < 1e-9);
        assert_eq!(slot.borrow().calls, 2);
    }

    #[test]
    fn usage_scope_nests_and_restores_the_active_frame() {
        let outer = open_usage_scope();
        let outer_slot = current_usage_accum().expect("outer scope active");
        {
            let _inner = open_usage_scope();
            let inner_slot = current_usage_accum().expect("inner scope active");
            assert!(
                !Rc::ptr_eq(&inner_slot, &outer_slot),
                "a nested scope installs a distinct frame"
            );
        }
        // inner dropped → the outer frame is the active one again
        assert!(
            Rc::ptr_eq(&current_usage_accum().expect("outer restored"), &outer_slot),
            "dropping the inner scope restores the outer frame"
        );
        drop(outer);
    }

    #[test]
    fn open_usage_scope_collects_completions_made_while_alive() {
        let scope = open_usage_scope();
        // Simulate two completions folding into the active frame (as track_usage does).
        let slot = current_usage_accum().expect("scope active");
        accumulate_into(&slot, &usage(20, 10), Some(0.01));
        accumulate_into(&slot, &usage(4, 2), None);
        let u = scope.usage();
        assert_eq!(u.input_tokens, 24);
        assert_eq!(u.output_tokens, 12);
        assert_eq!(u.calls, 2);
        drop(scope);
        assert!(
            current_usage_accum().is_none(),
            "dropping the only scope leaves no active frame"
        );
    }

    #[test]
    fn enforce_rate_limit_survives_backward_clock() {
        // A last-request timestamp in the future (wall clock jumped backward)
        // must not panic on the `now - last` subtraction (debug overflow check)
        // and must not produce a huge sleep.
        RATE_LIMIT_RPS.with(|r| r.set(Some(10.0)));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        RATE_LIMIT_LAST.with(|l| l.set(now + 1_000_000));
        let start = std::time::Instant::now();
        enforce_rate_limit();
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "backward clock should not cause a long sleep"
        );
        RATE_LIMIT_RPS.with(|r| r.set(None));
        RATE_LIMIT_LAST.with(|l| l.set(0));
    }

    #[test]
    fn url_host_extraction() {
        assert_eq!(
            url_host("https://api.openai.com/v1").as_deref(),
            Some("api.openai.com")
        );
        assert_eq!(
            url_host("http://localhost:11434").as_deref(),
            Some("localhost")
        );
        assert_eq!(
            url_host("http://user:pass@10.0.0.1:8080/x").as_deref(),
            Some("10.0.0.1")
        );
        assert_eq!(url_host("http://[::1]:9200/").as_deref(), Some("::1"));
        assert_eq!(
            url_host("http://169.254.169.254/latest").as_deref(),
            Some("169.254.169.254")
        );
    }

    #[test]
    fn internal_hosts_are_flagged() {
        for h in [
            "localhost",
            "app.localhost",
            "127.0.0.1",
            "0.0.0.0",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254", // cloud metadata
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1", // ipv4-mapped loopback
        ] {
            assert!(is_internal_host(h), "{h} should be internal");
        }
    }

    #[test]
    fn public_hosts_are_allowed() {
        for h in ["api.openai.com", "api.anthropic.com", "8.8.8.8", "1.1.1.1"] {
            assert!(!is_internal_host(h), "{h} should be allowed");
        }
    }

    #[test]
    fn internal_hosts_flagged_through_inet_aton_encodings() {
        // getaddrinfo accepts these and resolves them to loopback/internal,
        // but Ipv4Addr::from_str rejects them — the SSRF bypass.
        for h in [
            "2130706433", // decimal 127.0.0.1
            "0177.0.0.1", // octal first octet
            "0x7f.0.0.1", // hex first octet
            "0x7f000001", // single hex 32-bit 127.0.0.1
            "127.1",      // short form -> 127.0.0.1
            "127.0.0.1.", // trailing dot
            "0xA9FEA9FE", // 169.254.169.254 cloud metadata
        ] {
            assert!(is_internal_host(h), "{h} should be flagged internal");
        }
    }

    #[test]
    fn public_numeric_encodings_still_allowed() {
        // Numeric forms that decode to genuinely public addresses must not be
        // over-blocked (don't break legit numeric base-urls).
        for h in [
            "134744072",  // decimal 8.8.8.8
            "0x08080808", // hex 8.8.8.8
            "8.8.8.8.",   // trailing dot, public
            "010.0.0.1",  // octal 8.0.0.1 -> public
        ] {
            assert!(!is_internal_host(h), "{h} should be allowed (public)");
        }
    }

    #[test]
    fn guard_blocks_internal_only_when_sandboxed() {
        let mut opts = BTreeMap::new();
        opts.insert(
            Value::keyword("base-url"),
            Value::string("http://169.254.169.254/"),
        );
        // Unrestricted (normal CLI/REPL): allowed — local proxies / Ollama work.
        assert!(guard_provider_url(true, &opts).is_ok());
        // Sandboxed: rejected.
        assert!(guard_provider_url(false, &opts).is_err());

        let mut public_opts = BTreeMap::new();
        public_opts.insert(
            Value::keyword("base-url"),
            Value::string("https://api.openai.com/v1"),
        );
        assert!(guard_provider_url(false, &public_opts).is_ok());
    }

    fn make_lambda(params: &[&str]) -> Value {
        Value::lambda(Lambda {
            params: params.iter().map(|s| intern(s)).collect(),
            rest_param: None,
            body: vec![Value::nil()],
            env: Env::new(),
            name: None,
        })
    }

    fn make_param_map(keys: &[&str]) -> Value {
        let mut map = BTreeMap::new();
        for k in keys {
            map.insert(Value::keyword(k), Value::map(BTreeMap::new()));
        }
        Value::map(map)
    }

    // -- json_args_to_sema tests --

    #[test]
    fn test_json_args_to_sema_lambda_declaration_order() {
        // Params declared as (path, content) — but alphabetically content < path.
        // The lambda path must use declaration order, not alphabetical.
        let handler = make_lambda(&["path", "content"]);
        let params = make_param_map(&["path", "content"]);
        let args = json!({"path": "/tmp/test.txt", "content": "hello world"});

        let result = json_args_to_sema(&params, &args, &handler);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], Value::string("/tmp/test.txt"));
        assert_eq!(result[1], Value::string("hello world"));
    }

    #[test]
    fn test_json_args_to_sema_many_params_declaration_order() {
        // 4 params where alphabetical (a, b, c, d) != declaration order (d, b, a, c)
        let handler = make_lambda(&["delta", "bravo", "alpha", "charlie"]);
        let params = make_param_map(&["delta", "bravo", "alpha", "charlie"]);
        let args = json!({
            "alpha": "A",
            "bravo": "B",
            "charlie": "C",
            "delta": "D"
        });

        let result = json_args_to_sema(&params, &args, &handler);

        assert_eq!(result.len(), 4);
        assert_eq!(result[0], Value::string("D")); // delta first (declaration order)
        assert_eq!(result[1], Value::string("B")); // bravo second
        assert_eq!(result[2], Value::string("A")); // alpha third
        assert_eq!(result[3], Value::string("C")); // charlie fourth
    }

    #[test]
    fn test_json_args_to_sema_missing_arg_yields_nil() {
        let handler = make_lambda(&["path", "content"]);
        let params = make_param_map(&["path", "content"]);
        let args = json!({"path": "/tmp/test.txt"});

        let result = json_args_to_sema(&params, &args, &handler);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], Value::string("/tmp/test.txt"));
        assert_eq!(result[1], Value::nil());
    }

    #[test]
    fn test_json_args_to_sema_non_lambda_falls_back_to_btreemap() {
        // With a NativeFn handler, should fall back to param_map key order (alphabetical).
        let handler = Value::native_fn(NativeFn::simple("test", |_args| Ok(Value::nil())));
        let params = make_param_map(&["zebra", "apple"]);
        let args = json!({"zebra": "Z", "apple": "A"});

        let result = json_args_to_sema(&params, &args, &handler);

        // BTreeMap sorts alphabetically: apple < zebra
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], Value::string("A")); // apple first (alphabetical)
        assert_eq!(result[1], Value::string("Z")); // zebra second
    }

    #[test]
    fn test_json_args_to_sema_non_object_json() {
        let handler = make_lambda(&["x"]);
        let params = make_param_map(&["x"]);
        let args = json!("just a string");

        let result = json_args_to_sema(&params, &args, &handler);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0], Value::string("just a string"));
    }

    #[test]
    fn test_json_args_to_sema_mixed_types() {
        let handler = make_lambda(&["name", "age", "active"]);
        let params = make_param_map(&["name", "age", "active"]);
        let args = json!({"name": "Alice", "age": 30, "active": true});

        let result = json_args_to_sema(&params, &args, &handler);

        // Declaration order: name, age, active
        assert_eq!(result[0], Value::string("Alice"));
        assert_eq!(result[1], Value::int(30));
        assert_eq!(result[2], Value::bool(true));
    }

    // -- tool-call argument ordering (json_args_to_sema) --
    // These pin that JSON arguments bind to handler params by *declaration order*,
    // not alphabetically. The binding lives in `json_args_to_sema`; the handler is
    // applied later via the canonical evaluator callback (covered end-to-end by the
    // FakeProvider agent tests in `crates/sema/tests/llm_fake_test.rs`).

    #[test]
    fn test_tool_args_bind_in_declaration_order() {
        // Params (path, content): alphabetical order would swap them.
        let handler = Value::lambda(Lambda {
            params: vec![intern("path"), intern("content")],
            rest_param: None,
            body: vec![Value::symbol("path")],
            env: Env::new(),
            name: Some(intern("write-file-handler")),
        });
        let params = make_param_map(&["path", "content"]);
        let args = json!({"path": "/tmp/test.txt", "content": "file body here"});

        let result = json_args_to_sema(&params, &args, &handler);

        // Declaration order (path, content), not alphabetical (content, path).
        assert_eq!(result[0], Value::string("/tmp/test.txt"));
        assert_eq!(result[1], Value::string("file body here"));
    }

    #[test]
    fn test_tool_args_reverse_alpha_order() {
        // Params (z_last, a_first): exact reverse of alphabetical.
        let handler = Value::lambda(Lambda {
            params: vec![intern("z_last"), intern("a_first")],
            rest_param: None,
            body: vec![Value::symbol("z_last")],
            env: Env::new(),
            name: Some(intern("test-handler")),
        });
        let params = make_param_map(&["z_last", "a_first"]);
        let args = json!({"z_last": "ZLAST", "a_first": "AFIRST"});

        let result = json_args_to_sema(&params, &args, &handler);

        // z_last is declared first, so it must be arg 0 — not alphabetical.
        assert_eq!(result[0], Value::string("ZLAST"));
        assert_eq!(result[1], Value::string("AFIRST"));
    }

    #[test]
    fn test_validate_extraction_missing_key() {
        let schema = {
            let mut map = BTreeMap::new();
            let mut name_spec = BTreeMap::new();
            name_spec.insert(Value::keyword("type"), Value::keyword("string"));
            map.insert(Value::keyword("name"), Value::map(name_spec));
            let mut age_spec = BTreeMap::new();
            age_spec.insert(Value::keyword("type"), Value::keyword("number"));
            map.insert(Value::keyword("age"), Value::map(age_spec));
            Value::map(map)
        };
        let result = {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("name"), Value::string("Alice"));
            Value::map(map)
        };
        let err = validate_extraction(&result, &schema).unwrap_err();
        assert!(err.contains("missing key: age"), "got: {err}");
    }

    #[test]
    fn test_validate_extraction_wrong_type() {
        let schema = {
            let mut map = BTreeMap::new();
            let mut name_spec = BTreeMap::new();
            name_spec.insert(Value::keyword("type"), Value::keyword("string"));
            map.insert(Value::keyword("name"), Value::map(name_spec));
            Value::map(map)
        };
        let result = {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("name"), Value::int(42));
            Value::map(map)
        };
        let err = validate_extraction(&result, &schema).unwrap_err();
        assert!(err.contains("expected string"), "got: {err}");
    }

    #[test]
    fn test_validate_extraction_valid() {
        let schema = {
            let mut map = BTreeMap::new();
            let mut name_spec = BTreeMap::new();
            name_spec.insert(Value::keyword("type"), Value::keyword("string"));
            map.insert(Value::keyword("name"), Value::map(name_spec));
            Value::map(map)
        };
        let result = {
            let mut map = BTreeMap::new();
            map.insert(Value::keyword("name"), Value::string("Alice"));
            Value::map(map)
        };
        assert!(validate_extraction(&result, &schema).is_ok());
    }

    #[test]
    fn test_format_reask_prompt() {
        let prev_response = r#"{"name": 42}"#;
        let errors = "key name: expected string, got integer";
        let schema_desc = r#"{ "name": <string> }"#;
        let result = format_reask_prompt(prev_response, errors, schema_desc);
        assert!(result.contains("Previous response:"));
        assert!(result.contains(prev_response));
        assert!(result.contains(errors));
    }

    #[test]
    fn test_fallback_chain_thread_local() {
        FALLBACK_CHAIN.with(|chain| {
            assert!(chain.borrow().is_none());
            *chain.borrow_mut() = Some(vec![
                FallbackEntry {
                    provider: "openai".to_string(),
                    model: None,
                },
                FallbackEntry {
                    provider: "anthropic".to_string(),
                    model: None,
                },
            ]);
            assert_eq!(chain.borrow().as_ref().unwrap().len(), 2);
            *chain.borrow_mut() = None;
        });
    }

    #[test]
    fn test_parse_fallback_entry_bare_keyword() {
        let entry = parse_fallback_entry(&Value::keyword("anthropic")).unwrap();
        assert_eq!(entry.provider, "anthropic");
        assert_eq!(entry.model, None);
    }

    #[test]
    fn test_parse_fallback_entry_pair() {
        let v = Value::vector(vec![Value::keyword("openai"), Value::string("gpt-5.5")]);
        let entry = parse_fallback_entry(&v).unwrap();
        assert_eq!(entry.provider, "openai");
        assert_eq!(entry.model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn test_parse_fallback_entry_map() {
        let mut map = BTreeMap::new();
        map.insert(Value::keyword("provider"), Value::keyword("anthropic"));
        map.insert(Value::keyword("model"), Value::string("claude-opus-4-8"));
        let entry = parse_fallback_entry(&Value::map(map)).unwrap();
        assert_eq!(entry.provider, "anthropic");
        assert_eq!(entry.model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn test_parse_fallback_entry_bad_pair_len() {
        let v = Value::vector(vec![
            Value::keyword("openai"),
            Value::string("a"),
            Value::string("b"),
        ]);
        assert!(parse_fallback_entry(&v).is_err());
    }
}
