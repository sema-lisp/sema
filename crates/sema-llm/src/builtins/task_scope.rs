use super::*;

pub(super) struct CassetteState {
    pub(super) cassette: RefCell<Option<crate::cassette::Cassette>>,
}

impl CassetteState {
    pub(super) fn new(cassette: crate::cassette::Cassette) -> Self {
        Self {
            cassette: RefCell::new(Some(cassette)),
        }
    }

    pub(super) fn borrow(&self) -> std::cell::Ref<'_, crate::cassette::Cassette> {
        std::cell::Ref::map(self.cassette.borrow(), |cassette| {
            cassette
                .as_ref()
                .expect("live cassette scope always owns its tape")
        })
    }

    pub(super) fn borrow_mut(&self) -> std::cell::RefMut<'_, crate::cassette::Cassette> {
        std::cell::RefMut::map(self.cassette.borrow_mut(), |cassette| {
            cassette
                .as_mut()
                .expect("live cassette scope always owns its tape")
        })
    }
}

impl Drop for CassetteState {
    fn drop(&mut self) {
        if let Some(cassette) = self.cassette.get_mut().as_mut() {
            persist_cassette_off_quantum(cassette);
        }
    }
}

impl sema_core::McpCassetteRecordTarget for CassetteState {
    fn record(&self, key: &str, value: &serde_json::Value) {
        self.borrow_mut()
            .record_entry(crate::cassette::TapeEntry::from_mcp_call(key, value));
    }
}

pub(super) type CassetteScope = Rc<CassetteState>;

/// Declares [`LlmDynScope`]'s struct, `Default` impl, and its `read_llm_scope`/
/// `write_llm_scope` round-trip from ONE field table, so a new per-task LLM
/// config field is added in exactly one place instead of the 3-4 sites that
/// have to be kept in lockstep by hand (a defect this codebase has shipped at
/// least 3 times — commits b181d195, eec95fb4, 0a58f9ac — most recently when
/// `custom_pricing`/`budget_stack` were left as ambient TLS entirely, letting a
/// sibling task's `(llm/set-pricing ...)` reprice a suspended task's usage).
/// `llm_dyn_scope_is_default`/`llm_scope_ambient_is_empty` are hand-written and
/// NOT generated here: their exclusion of `stream_budget_pregate`/`cache_ttl_secs`/
/// `rate_limit_last` from the default-check is deliberate and asymmetric with
/// this table, and `llm_scope_ambient_is_empty` is a documented allocation-free
/// fast path that must stay hand-optimized.
macro_rules! llm_dyn_scope_fields {
    ($($(#[$meta:meta])* $field:ident : $ty:ty = default $default:expr, read $read:expr, write $write:expr;)*) => {
        /// The dynamically-scoped LLM state captured onto a task and swapped in/out per step.
        #[derive(Clone)]
        pub(super) struct LlmDynScope {
            $($(#[$meta])* $field: $ty,)*
        }

        impl Default for LlmDynScope {
            fn default() -> Self {
                LlmDynScope { $($field: $default,)* }
            }
        }

        /// Read (clone) the current thread's LLM dynamic scope without disturbing it.
        pub(super) fn read_llm_scope() -> LlmDynScope {
            LlmDynScope { $($field: $read,)* }
        }

        /// Overwrite the current thread's LLM dynamic scope with `s`, returning the previous one.
        pub(super) fn write_llm_scope(s: LlmDynScope) -> LlmDynScope {
            let prev = read_llm_scope();
            $(($write)(s.$field);)*
            prev
        }
    };
}

llm_dyn_scope_fields! {
    cache_enabled: bool
        = default false, read CACHE_ENABLED.with(|c| c.get()), write |v| CACHE_ENABLED.with(|c| c.set(v));
    cache_ttl_secs: i64
        = default 3600, read CACHE_TTL_SECS.with(|c| c.get()), write |v| CACHE_TTL_SECS.with(|c| c.set(v));
    stream_budget_pregate: bool
        = default false, read STREAM_BUDGET_PREGATE.with(|c| c.get()), write |v| STREAM_BUDGET_PREGATE.with(|c| c.set(v));
    call_tags: Vec<String>
        = default Vec::new(), read CALL_TAGS.with(|t| t.borrow().clone()), write |v| CALL_TAGS.with(|t| *t.borrow_mut() = v);
    call_meta: Vec<(String, String)>
        = default Vec::new(), read CALL_META.with(|m| m.borrow().clone()), write |v| CALL_META.with(|m| *m.borrow_mut() = v);
    last_usage: Option<Usage>
        = default None, read LAST_USAGE.with(|u| u.borrow().clone()), write |v| LAST_USAGE.with(|u| *u.borrow_mut() = v);
    fallback_chain: Option<Vec<FallbackEntry>>
        = default None, read FALLBACK_CHAIN.with(|c| c.borrow().clone()), write |v| FALLBACK_CHAIN.with(|c| *c.borrow_mut() = v);
    rate_limit_rps: Option<f64>
        = default None, read RATE_LIMIT_RPS.with(|r| r.get()), write |v| RATE_LIMIT_RPS.with(|r| r.set(v));
    /// Siblings spawned inside one rate-limit scope reserve against one cursor.
    rate_limit_last: Option<Rc<Cell<u64>>>
        = default None, read RATE_LIMIT_LAST.with(|last| last.borrow().clone()), write |v| RATE_LIMIT_LAST.with(|last| *last.borrow_mut() = v);
    retry_base_ms: u64
        = default 500, read RETRY_BASE_MS.with(|base| base.get()), write |v| RETRY_BASE_MS.with(|base| base.set(v));
    network_max_retries: u32
        = default 3, read NETWORK_MAX_RETRIES.with(|retries| retries.get()), write |v| NETWORK_MAX_RETRIES.with(|retries| retries.set(v));
    /// The active budget frame, shared by `Rc` so concurrent siblings charge one aggregate.
    budget: Option<Rc<RefCell<BudgetFrame>>>
        = default None, read ACTIVE_BUDGET.with(|b| b.borrow().clone()), write |v| ACTIVE_BUDGET.with(|b| *b.borrow_mut() = v);
    /// Saved outer budget frames for nested `llm/with-budget` scopes (TASK-PRIVATE
    /// bookkeeping). A push saves the frame in force; the matching pop restores it.
    /// Parking this stack onto the task is what lets interleaved nested budget scopes
    /// each restore their OWN outer frame — an ambient stack shared across suspensions
    /// would pop a sibling's frame out of LIFO order. The saved frames are shared by
    /// `Rc` (a restored outer frame is the same aggregate), but the stack structure is
    /// per-task.
    budget_stack: Vec<Option<Rc<RefCell<BudgetFrame>>>>
        = default Vec::new(), read BUDGET_STACK.with(|s| s.borrow().clone()), write |v| BUDGET_STACK.with(|stack| *stack.borrow_mut() = v);
    /// Custom per-model pricing overrides (TASK-SNAPSHOT config). Parked onto the task so
    /// a sibling's `(llm/set-pricing ...)` never reprices a suspended task's usage.
    custom_pricing: std::collections::HashMap<String, (f64, f64)>
        = default std::collections::HashMap::new(), read pricing::snapshot_custom_pricing(), write pricing::restore_custom_pricing;
    /// The cassette selected by this scope. Spawned siblings share one tape so
    /// replay and recording remain coherent across quantum boundaries.
    cassette: Option<CassetteScope>
        = default None, read CASSETTE.with(|c| c.borrow().clone()), write |v| CASSETTE.with(|c| *c.borrow_mut() = v);
    policies: Vec<ActivePolicy>
        = default Vec::new(), read ACTIVE_POLICIES.with(|policies| policies.borrow().clone()), write |v| ACTIVE_POLICIES.with(|policies| *policies.borrow_mut() = v);
    policy_bypass: Vec<String>
        = default Vec::new(), read POLICY_BYPASS.with(|bypass| bypass.borrow().clone()), write |v| POLICY_BYPASS.with(|bypass| *bypass.borrow_mut() = v);
    policy_agent_id: Option<String>
        = default None, read POLICY_AGENT_ID.with(|agent| agent.borrow().clone()), write |v| POLICY_AGENT_ID.with(|agent| *agent.borrow_mut() = v);
}

/// Capture (clone) the LLM dynamic scope to seed onto a freshly-spawned task.
pub(super) fn capture_llm_scope() -> Box<dyn std::any::Any> {
    let mut scope = read_llm_scope();
    // `last-usage` describes the most recent request made by this task. A new
    // task inherits dynamic configuration, but starts without request history.
    scope.last_usage = None;
    // The budget save-stack is TASK-PRIVATE lexical bookkeeping: a child inherits the
    // ACTIVE budget frame (`budget`, shared by `Rc`, so a fan-out charges one aggregate)
    // but starts its own nesting fresh — like `last_usage`, its own `with-budget` scopes
    // push/pop against an empty stack.
    scope.budget_stack = Vec::new();
    Box::new(scope)
}

/// Take the LLM dynamic scope out of the thread-locals, leaving defaults.
pub(super) fn take_llm_scope() -> Box<dyn std::any::Any> {
    Box::new(write_llm_scope(LlmDynScope::default()))
}

/// Install an LLM dynamic scope into the thread-locals, returning the one displaced.
pub(super) fn install_llm_scope(ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
    let incoming: LlmDynScope = ctx
        .downcast::<LlmDynScope>()
        .map(|b| *b)
        .unwrap_or_default();
    Box::new(write_llm_scope(incoming))
}

/// Fast-path predicate (`TaskScopeSwap`, sema-vm `state.rs`): a captured LLM
/// dynamic scope is empty when it carries no cache/budget/cassette/tag overrides — i.e.
/// it is bytewise the same as [`LlmDynScope::default`]. No allocation (field
/// reads only, no clone).
pub(super) fn llm_scope_captured_is_empty(ctx: &Box<dyn std::any::Any>) -> bool {
    match ctx.downcast_ref::<LlmDynScope>() {
        Some(s) => llm_dyn_scope_is_default(s),
        None => true,
    }
}

/// Peek (no mutation, no allocation) whether the thread-local LLM dynamic scope
/// is currently at its default (no cache/budget/cassette/tag overrides active).
pub(super) fn llm_scope_ambient_is_empty() -> bool {
    !CACHE_ENABLED.with(Cell::get)
        && !ACTIVE_BUDGET.with(|b| b.borrow().is_some())
        && BUDGET_STACK.with(|s| s.borrow().is_empty())
        && pricing::custom_pricing_is_empty()
        && CALL_TAGS.with(|t| t.borrow().is_empty())
        && CALL_META.with(|m| m.borrow().is_empty())
        && LAST_USAGE.with(|u| u.borrow().is_none())
        && FALLBACK_CHAIN.with(|c| c.borrow().is_none())
        && RATE_LIMIT_RPS.with(|r| r.get().is_none())
        && RETRY_BASE_MS.with(|base| base.get() == 500)
        && NETWORK_MAX_RETRIES.with(|retries| retries.get() == 3)
        && CASSETTE.with(|c| c.borrow().is_none())
        && ACTIVE_POLICIES.with(|policies| policies.borrow().is_empty())
        && POLICY_BYPASS.with(|bypass| bypass.borrow().is_empty())
        && POLICY_AGENT_ID.with(|agent| agent.borrow().is_none())
}

/// Shared field-by-field default check for [`LlmDynScope`] (avoids requiring
/// `PartialEq`/cloning just to compare against `Default::default()`). Ignores
/// `cache_ttl_secs`/`stream_budget_pregate` when the flags they gate are off —
/// a stray `llm/set-cache-ttl` with caching disabled still counts as empty
/// (correctness-safe: it means the fast path is skipped slightly less often,
/// never more).
pub(super) fn llm_dyn_scope_is_default(s: &LlmDynScope) -> bool {
    !s.cache_enabled
        && s.budget.is_none()
        && s.budget_stack.is_empty()
        && s.custom_pricing.is_empty()
        && s.cassette.is_none()
        && s.call_tags.is_empty()
        && s.call_meta.is_empty()
        && s.last_usage.is_none()
        && s.fallback_chain.is_none()
        && s.rate_limit_rps.is_none()
        && s.retry_base_ms == 500
        && s.network_max_retries == 3
        && s.policies.is_empty()
        && s.policy_bypass.is_empty()
        && s.policy_agent_id.is_none()
}

/// Register the per-task LLM dynamic-scope callbacks with sema-core. Called once at startup.
pub fn register_llm_scope_task_callbacks() {
    sema_core::set_llm_scope_task_callbacks(capture_llm_scope, take_llm_scope, install_llm_scope);
    sema_core::set_llm_scope_empty_callbacks(
        llm_scope_captured_is_empty,
        llm_scope_ambient_is_empty,
    );
}

thread_local! {
    pub(super) static PRICING_WARNING_SHOWN: Cell<bool> = const { Cell::new(false) };
    pub(super) static LISP_PROVIDERS: RefCell<std::collections::HashMap<String, LispProviderCallbacks>> = RefCell::new(std::collections::HashMap::new());
    pub(super) static CACHE_ENABLED: Cell<bool> = const { Cell::new(false) };
    pub(super) static CACHE_MEM: RefCell<std::collections::HashMap<String, CachedResponse>> =
        RefCell::new(std::collections::HashMap::new());
    pub(super) static CACHE_TTL_SECS: Cell<i64> = const { Cell::new(3600) };
    pub(super) static CACHE_HITS: Cell<u64> = const { Cell::new(0) };
    pub(super) static CACHE_MISSES: Cell<u64> = const { Cell::new(0) };
    // Active LLM cassette (record/replay). The shared scope is captured onto
    // spawned tasks with the rest of `LlmDynScope`.
    pub(super) static CASSETTE: RefCell<Option<CassetteScope>> = const { RefCell::new(None) };
    pub(super) static FALLBACK_CHAIN: RefCell<Option<Vec<FallbackEntry>>> = const { RefCell::new(None) };
    pub(super) static VECTOR_STORES: RefCell<std::collections::HashMap<String, VectorStore>> =
        RefCell::new(std::collections::HashMap::new());
    pub(super) static RATE_LIMIT_RPS: Cell<Option<f64>> = const { Cell::new(None) };
    pub(super) static RATE_LIMIT_LAST: RefCell<Option<Rc<Cell<u64>>>> = const { RefCell::new(None) };
    // Name of the provider that served the most recent `do_complete` response, so cost
    // tracking can price the model as served by that provider (resellers/gateways can list
    // the same model id at a different rate). Set at the dispatch choke points, consumed +
    // cleared by `track_usage`. `None` → canonical first-party price.
    pub(super) static LAST_SERVING_PROVIDER: RefCell<Option<String>> = const { RefCell::new(None) };
}
