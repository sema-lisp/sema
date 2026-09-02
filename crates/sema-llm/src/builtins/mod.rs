use std::cell::Cell;
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;

use sema_core::runtime::RuntimeTaskId;
use sema_core::{
    resolve, Agent, ArgsExt, Conversation, Env, EvalContext, ImageAttachment, Message, NativeFn,
    OptionsExt, PolicyDenial, Prompt, ResultExt, Role, SemaError, Value, ValueView,
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
    MessageContent, RerankRequest, RerankResponse, ToolCall, ToolSchema, Usage,
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
    /// loop sums every round's usage instead of seeing only the last. Runtime
    /// completion paths capture this frame's `Rc` into their decoder so the fold
    /// lands outside the task-step boundary.
    static ACTIVE_LEAF_SCOPE: RefCell<Option<Rc<RefCell<LeafUsage>>>> = const { RefCell::new(None) };
    /// Set while a runtime completion decoder folds usage into a captured frame `Rc`.
    /// Suppresses `track_usage`'s own active-frame fold so the runtime path counts each
    /// completion exactly once.
    static USAGE_ACCUM_SUPPRESS: Cell<bool> = const { Cell::new(false) };
    static SESSION_COST: RefCell<f64> = const { RefCell::new(0.0) };
    /// The budget frame in force for the CURRENT TASK, held behind a shared `Rc` so
    /// that all concurrent tasks spawned inside one `llm/with-budget` charge ONE
    /// aggregate frame (captured by-`Rc` onto each task at spawn via the per-task LLM
    /// dynamic-scope seam, and re-installed around the runtime completion decoder's
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
    /// Ordered policy layers active for the current task. The LLM
    /// dynamic-scope mechanism captures and swaps these with cache/budget/cassette
    /// state, so workflow and step policies remain isolated across sibling tasks.
    static ACTIVE_POLICIES: RefCell<Vec<ActivePolicy>> = const { RefCell::new(Vec::new()) };
    /// Trusted lexical policy bypass reasons. A nonempty stack suppresses policy
    /// enforcement but still emits a `policy.bypassed` observation per boundary.
    static POLICY_BYPASS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Workflow step attribution carried with the policy scope, independent of
    /// the workflow crate's task-local state.
    static POLICY_AGENT_ID: RefCell<Option<String>> = const { RefCell::new(None) };
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

mod agent;
mod batch;
mod cache;
mod cassette_bridge;
mod chat;
mod complete;
mod conversation;
mod convert;
mod embed;
mod extract;
mod lisp_provider;
mod opts;
mod policy;
mod registry;
mod state;
mod stream;
mod task_scope;
mod telemetry;
#[cfg(test)]
mod tests;
mod usage;
mod vectors;

use agent::*;
use cache::*;
use cassette_bridge::*;
use chat::*;
use complete::*;
use conversation::*;
use convert::*;
use embed::*;
use extract::*;
use lisp_provider::*;
use opts::*;
use policy::*;
use registry::*;
use state::*;
use stream::*;
use task_scope::*;
use telemetry::*;
use usage::*;

pub use agent::agent_runs_len;
pub use agent::stream_runs_len;
pub use cassette_bridge::install_cassette;
pub use cassette_bridge::take_cassette;
pub use complete::set_network_max_retries;
pub use complete::set_retry_base_ms;
pub use policy::effective_policy_fingerprint;
pub use policy::open_policy_attribution;
pub use policy::open_policy_bypass;
pub use policy::open_policy_scopes;
pub use policy::policy_active;
pub use policy::PolicyAttributionScope;
pub use policy::PolicyBoundary;
pub use policy::PolicyBypassScope;
pub use policy::PolicyDecisionSink;
pub use policy::PolicyObservation;
pub use policy::PolicyObservationKind;
pub use policy::PolicyScope;
pub use policy::PolicySource;
#[cfg(not(target_arch = "wasm32"))]
pub use state::io_peak_inflight;
pub(crate) use state::note_off_quantum_fs;
pub use state::quantum_fs_calls;
pub use state::register_test_provider;
#[cfg(not(target_arch = "wasm32"))]
pub use state::reset_io_inflight;
pub use state::reset_quantum_fs_calls;
pub use state::reset_runtime_state;
pub use state::session_cost_snapshot;
#[cfg(not(target_arch = "wasm32"))]
pub use state::IO_INFLIGHT;
#[cfg(not(target_arch = "wasm32"))]
pub use state::IO_PEAK;
pub use task_scope::register_llm_scope_task_callbacks;
pub use usage::clear_budget;
pub use usage::clear_last_usage;
pub use usage::last_usage_snapshot;
pub use usage::open_usage_scope;
pub use usage::pop_budget_scope;
pub use usage::push_budget_scope;
pub use usage::register_usage_scope_task_callbacks;
pub use usage::set_budget;
pub use usage::set_token_budget;
pub use usage::LastUsage;
pub use usage::LeafUsage;
pub use usage::UsageScope;

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

    // Bridge the task-scoped LLM cassette to MCP tool calls through sema-core.
    // Idempotent: interpreter setup replaces one thread-local function pointer.
    sema_core::set_mcp_cassette_hook(mcp_cassette_decide);

    // Reclaim non-blocking agent-run slab entries owned by a CANCELLED task
    // (whose `__agent-finish` can never run) the moment the scheduler reaps it —
    // ending the agent span balanced on the VM thread instead of leaking the
    // entry (and its telemetry) until `reset_runtime_state`. Same type-erased
    // fn-pointer seam as the callbacks above; idempotent. Invariant I2 holds:
    // a plain `fn`, captures nothing.
    sema_core::set_task_reaped_callback(reap_cancelled_agent_runs);

    // CI cassette baseline: SEMA_LLM_CASSETTE=path
    // [+ SEMA_LLM_CASSETTE_MODE=replay|record|auto] installs a cassette in the
    // current ambient LLM scope, so tasks spawned afterward inherit deterministic
    // replay without touching test source. Only honored outside the sandbox (it
    // reads/writes a file path from the environment).
    if unrestricted {
        if let Ok(path) = std::env::var("SEMA_LLM_CASSETTE") {
            if !path.is_empty() {
                let mode = std::env::var("SEMA_LLM_CASSETTE_MODE")
                    .map(|s| crate::cassette::CassetteMode::parse(&s))
                    .unwrap_or(crate::cassette::CassetteMode::Auto);
                let cassette =
                    crate::cassette::Cassette::load(std::path::PathBuf::from(path), mode);
                install_cassette(cassette);
            }
        }
    }

    chat::register(env, sandbox, unrestricted);
    stream::register(env);
    extract::register(env, sandbox);
    conversation::register(env);
    usage::register(env);
    agent::register(env, sandbox);
    batch::register(env);
    embed::register(env, unrestricted);
    cache::register(env);
    cassette_bridge::register(env);
    complete::register(env);
    vectors::register(env);
}
