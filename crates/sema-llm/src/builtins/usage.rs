use super::*;

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
    /// Prompt tokens served from the provider's prompt cache. See `Usage::
    /// cache_read_input_tokens` for the per-provider accounting caveat.
    pub cache_read_input_tokens: u64,
    /// Tokens written to the prompt cache. See `Usage::cache_creation_input_tokens`.
    pub cache_creation_input_tokens: u64,
    /// Summed cost; `None` while no priced call has landed, `Some` once one has (a
    /// later unpriced call leaves the running sum unchanged).
    pub cost_usd: Option<f64>,
    /// The model of the most recent priced/recorded call in this scope.
    pub model: String,
    pub calls: u32,
    /// Completions served from `llm/with-cache` (all-zero usage). Not counted
    /// in `calls`, so `calls == 0` gates still treat a purely-cached leaf as
    /// free, while consumers can distinguish "cached, $0" from "no LLM call".
    pub cache_hits: u32,
}

/// Fold one completion's usage into a `LeafUsage` tally. A cache hit (all-zero usage)
/// does NOT increment `calls`, so a purely-cached leaf stays filtered downstream.
pub(super) fn accumulate_into(slot: &Rc<RefCell<LeafUsage>>, usage: &Usage, cost: Option<f64>) {
    let input = usage.prompt_tokens as u64;
    let output = usage.completion_tokens as u64;
    // A cache hit reports no tokens and no cost. Priced models can report the
    // cost as `Some(0.0)`, so `cost.is_none()` cannot identify cache hits.
    if input == 0 && output == 0 && cost.unwrap_or(0.0) == 0.0 {
        slot.borrow_mut().cache_hits += 1;
        return;
    }
    let mut acc = slot.borrow_mut();
    acc.input_tokens += input;
    acc.output_tokens += output;
    acc.cache_read_input_tokens += usage.cache_read_input_tokens as u64;
    acc.cache_creation_input_tokens += usage.cache_creation_input_tokens as u64;
    if let Some(c) = cost {
        acc.cost_usd = Some(acc.cost_usd.unwrap_or(0.0) + c);
    }
    if !usage.model.is_empty() {
        acc.model = usage.model.clone();
    }
    acc.calls += 1;
}

/// Fold a child leaf's tally into a parent's, used when a nested [`UsageScope`] drops
/// so the outer scope it shadowed isn't left blind to tokens the inner one collected.
pub(super) fn merge_leaf(dst: &mut LeafUsage, src: &LeafUsage) {
    if src.calls == 0 && src.cache_hits == 0 {
        return;
    }
    dst.cache_hits += src.cache_hits;
    dst.input_tokens += src.input_tokens;
    dst.output_tokens += src.output_tokens;
    dst.cache_read_input_tokens += src.cache_read_input_tokens;
    dst.cache_creation_input_tokens += src.cache_creation_input_tokens;
    if let Some(c) = src.cost_usd {
        dst.cost_usd = Some(dst.cost_usd.unwrap_or(0.0) + c);
    }
    if !src.model.is_empty() {
        dst.model = src.model.clone();
    }
    dst.calls += src.calls;
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
        // Nesting (e.g. `agent/run` opening its own scope inside an already-active
        // `workflow/step` scope) must not blind the outer scope to tokens the inner
        // one collected: fold this leaf's tally into the parent before restoring it.
        if let Some(parent) = &self.prev {
            merge_leaf(&mut parent.borrow_mut(), &self.slot.borrow());
        }
        ACTIVE_LEAF_SCOPE.with(|s| *s.borrow_mut() = self.prev.take());
    }
}

/// Open a per-leaf usage accumulation scope. `track_usage` folds each completion made
/// while the returned guard is alive into this scope's frame; the runtime completion path
/// captures the frame's `Rc` into its decoder so an in-flight leaf is tallied even after
/// a sibling task runs. The guard restores the prior active scope on drop.
pub fn open_usage_scope() -> UsageScope {
    let slot = Rc::new(RefCell::new(LeafUsage::default()));
    let prev = ACTIVE_LEAF_SCOPE.with(|s| s.borrow_mut().replace(Rc::clone(&slot)));
    UsageScope { slot, prev }
}

/// Clone the active (per-task) usage-accumulator frame's `Rc`, if any. The runtime
/// completion path captures this at dispatch so the decoder folds usage into the
/// LEAF'S OWN frame — correct even across a concurrent sibling task.
pub(super) fn current_usage_accum() -> Option<Rc<RefCell<LeafUsage>>> {
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
pub(super) fn capture_usage_scope() -> Box<dyn std::any::Any> {
    Box::new(ACTIVE_LEAF_SCOPE.with(|s| s.borrow().clone()))
}

/// Take the active leaf scope out of the thread-local (leaving none).
pub(super) fn take_usage_scope() -> Box<dyn std::any::Any> {
    Box::new(ACTIVE_LEAF_SCOPE.with(|s| s.borrow_mut().take()))
}

/// Install a leaf scope into the thread-local, returning the one displaced.
pub(super) fn install_usage_scope(ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
    let incoming: Option<Rc<RefCell<LeafUsage>>> = ctx
        .downcast::<Option<Rc<RefCell<LeafUsage>>>>()
        .map(|b| *b)
        .unwrap_or(None);
    Box::new(ACTIVE_LEAF_SCOPE.with(|s| std::mem::replace(&mut *s.borrow_mut(), incoming)))
}

/// Fast-path predicate (`TaskScopeSwap`, sema-vm `state.rs`): a captured usage
/// scope is empty when no leaf-usage accumulator is active. No allocation.
pub(super) fn usage_scope_captured_is_empty(ctx: &Box<dyn std::any::Any>) -> bool {
    ctx.downcast_ref::<Option<Rc<RefCell<LeafUsage>>>>()
        .is_none_or(Option::is_none)
}

/// Peek (no mutation, no allocation) whether the thread-local active leaf-usage
/// scope is currently empty.
pub(super) fn usage_scope_ambient_is_empty() -> bool {
    ACTIVE_LEAF_SCOPE.with(|s| s.borrow().is_none())
}

/// Register the per-task usage-scope callbacks with sema-core. Called once at startup.
pub fn register_usage_scope_task_callbacks() {
    sema_core::set_usage_scope_task_callbacks(
        capture_usage_scope,
        take_usage_scope,
        install_usage_scope,
    );
    sema_core::set_usage_scope_empty_callbacks(
        usage_scope_captured_is_empty,
        usage_scope_ambient_is_empty,
    );
}

// ── Per-task LLM dynamic scope (cache / budget / cassette / tags) ────
//
// Dynamic LLM builtins install thread-local state for the extent of a thunk. A spawned
// task may run after that extent has unwound, so the scheduler captures the scope at
// `async/spawn` and swaps it around every task step. Read-only flags ride as value
// snapshots; budget and cassette state use shared `Rc`s so siblings in one scope charge
// one aggregate or record into one tape. Reached from `sema-core` through the
// type-erased fn-pointer seam.

#[derive(Clone, Default)]
pub(super) struct BudgetFrame {
    pub(super) cost_limit: Option<f64>,
    pub(super) cost_spent: f64,
    pub(super) token_limit: Option<u64>,
    pub(super) tokens_spent: u64,
}

pub(super) fn track_usage(usage: &Usage) -> Result<(), SemaError> {
    // Price the model as served by the provider that produced this response (falls back to
    // the canonical first-party price when the serving provider is unknown).
    let provider = take_serving_provider().unwrap_or_default();
    let cost = pricing::calculate_cost_for(&provider, usage);
    let total_tokens = (usage.prompt_tokens + usage.completion_tokens) as u64;

    LAST_USAGE.with(|u| *u.borrow_mut() = Some(usage.clone()));
    // Fold into the active per-task leaf accumulator for the workflow runtime. SUMS
    // every round of a multi-round tool loop; cache hits (all-zero) don't bump `calls`.
    // The runtime decoder captures the leaf's own frame Rc and folds there instead, so it
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
/// this via `track_usage`; the runtime completion decoder captures it at dispatch and
/// re-installs it around its own `track_usage` so the charge lands on the frame that
/// was active when the completion was DISPATCHED, not whatever is active when the
/// future resolves.
pub(super) fn active_budget() -> Option<Rc<RefCell<BudgetFrame>>> {
    ACTIVE_BUDGET.with(|b| b.borrow().clone())
}

/// Ensure an active budget frame exists (creating an unbounded one if none), returning
/// its `Rc`. Used by the non-scoped `llm/set-budget`/`llm/set-token-budget` API.
pub(super) fn ensure_active_budget() -> Rc<RefCell<BudgetFrame>> {
    ACTIVE_BUDGET.with(|b| {
        b.borrow_mut()
            .get_or_insert_with(|| Rc::new(RefCell::new(BudgetFrame::default())))
            .clone()
    })
}

/// Charge `total_tokens` / `cost` into `frame` and return `Err` if either limit is now
/// exceeded. Cost is charged only when known (`Some`); the token charge always applies.
/// Shared by synchronous and runtime completion paths so both gate identically.
pub(super) fn charge_budget_frame(
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

pub(super) fn register(env: &Env) {
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

    // (llm/set-pricing "model-pattern" input-per-million output-per-million)
    register_fn(env, "llm/set-pricing", |args| {
        if args.len() != 3 {
            return Err(SemaError::arity("llm/set-pricing", "3", args.len()));
        }
        let model_pattern = args.str_at(0, "llm/set-pricing")?;
        let input_cost = args.float_at(1, "llm/set-pricing")?;
        let output_cost = args.float_at(2, "llm/set-pricing")?;
        pricing::set_custom_pricing(model_pattern, input_cost, output_cost);
        Ok(Value::nil())
    });

    register_fn(env, "llm/reset-usage", |_args| {
        SESSION_USAGE.with(|u| *u.borrow_mut() = Usage::default());
        LAST_USAGE.with(|u| *u.borrow_mut() = None);
        SESSION_COST.with(|sc| *sc.borrow_mut() = 0.0);
        Ok(Value::nil())
    });

    // (llm/set-budget max-cost-usd) — set a budget limit
    register_fn(env, "llm/set-budget", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("llm/set-budget", "1", args.len()));
        }
        let max_cost = args.float_at(0, "llm/set-budget")?;
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
    register_scope_fn_ctx(env, "llm/with-budget", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("llm/with-budget", "2", args.len()));
        }
        let opts = args.map_at(0, "llm/with-budget")?;
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

        // A fresh budget frame rides as a shared `Rc` (see `push_budget_scope`): a
        // concurrent fan-out spawned inside this scope captures the frame at spawn
        // and charges it as one aggregate. The frame is popped when the thunk
        // returns (its extent), restoring any outer `with-budget` frame.
        push_budget_scope(max_cost, max_tokens);
        let prev_pregate = STREAM_BUDGET_PREGATE.with(|c| c.replace(pregate));
        Ok((
            body_fn.clone(),
            Box::new(move || {
                STREAM_BUDGET_PREGATE.with(|c| c.set(prev_pregate));
                pop_budget_scope();
            }),
        ))
    });

    // --- Cache builtins ---
}
