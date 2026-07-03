# ASYNC-1 — Per-task capture of LLM dynamic scope (cache / budget / tags)

Closes the deferred item **ASYNC-1** (`docs/deferred.md`): `llm/with-cache`,
`llm/with-budget`, and per-call `:tags`/`:metadata` set **dynamically-scoped
thread-locals** in `crates/sema-llm/src/builtins.rs` for the extent of a thunk,
then reset them. An async task spawned inside that thunk reads those flags **when
it actually executes** — which the cooperative scheduler can defer past the point
where the thunk returned and the flag was reset. Two symptoms:

- **Visibility/accounting** (Scope A): a completion in a deferred task runs with
  `CACHE_ENABLED` already reset → `(llm/cache-stats)` under-reports and the
  `async_cache_miss_is_counted` gate was removed as flaky
  (`crates/sema/tests/complete_async_test.rs:80-89`). Tags/metadata attribution
  has the same shape.
- **Budget correctness** (Scope B): the same mechanism means `llm/with-budget`
  does **not** reliably gate concurrent completions — a fan-out inside a
  `with-budget` thunk can overshoot because each deferred completion charges
  against whatever budget frame happens to be installed when it resolves, not the
  frame that was active when it was dispatched.

Decision (locked with owner): **implement both A and B.** ADR #67.

## Root cause (one sentence)

The LLM dynamic-scope thread-locals are read at **task-execution / future-resolution**
time, not captured at `async/spawn` time — so they leak across the cooperative
scheduler's task-deferral boundary.

Read sites confirmed: `CACHE_ENABLED` at `builtins.rs:5866`; budget charge/check in
`track_usage` at `builtins.rs:536-577`; the async poller charging budget at
`builtins.rs:6232-6237`; the streaming pre-gate at `builtins.rs:6819-6842`;
tags/metadata applied at `builtins.rs:5793-5822`.

## The established fix pattern (already shipped, twice)

The scheduler already swaps two per-task thread-local contexts in on task entry and
out on leave, isolating concurrent tasks:

1. **OTel context** — `Task.otel` (`scheduler.rs:67`), captured at spawn
   (`current_conversation_scope_boxed`, line 157 / 514), installed on entry
   (`install_task_otel`, line 1061), taken back out + restored on leave
   (`ReinstallGuard::restore_otel`, lines 698-721).
2. **Usage scope** — `Task.usage_scope` (`scheduler.rs:73`), same lifecycle,
   registered by sema-llm via `register_usage_scope_task_callbacks()`
   (`builtins.rs:224-230`), reaching sema-core through the type-erased fn-pointer
   seam in `crates/sema-core/src/async_signal.rs:417-476`.

Additionally, the **async completion poller captures the leaf's usage-accumulator
`Rc` at yield time** (`usage_accum_slot = current_usage_accum()`,
`builtins.rs:6172`) and folds into that captured frame when the future lands
(`builtins.rs:6226-6229`), setting `USAGE_ACCUM_SUPPRESS` so `track_usage` doesn't
double-count. **This is the exact template for the budget fix.**

**The ASYNC-1 fix is a third per-task context of the same shape**, plus giving
budget the poller-capture treatment usage already has.

## Design

Introduce **one** per-task context — `LlmDynScope` — owned by sema-llm, bundling
every `with-*` dynamic-scope value. Two kinds of field:

- **Read-only snapshot fields** (value-copied per task): `cache_enabled: bool`,
  `cache_ttl_secs: i64`, `call_tags: Vec<String>`, `call_meta: Vec<(String,String)>`,
  `stream_budget_pregate: bool`.
- **Shared-accumulator field** (`Rc`, aggregated across siblings):
  `budget: Option<Rc<RefCell<BudgetFrame>>>` — the **active budget frame**.

> Out of the initial cut, documented as follow-up: `FALLBACK_CHAIN`, `RATE_LIMIT_*`,
> and `CASSETTE` are also dynamically scoped. They are NOT part of the ASYNC-1
> report and each has its own subtleties (a cassette recorder is a shared handle,
> not a value). Snapshotting them is a separate, additive change — call it out in
> the ADR's "out of scope" and leave a `// ASYNC-1 follow-up` note at each site.

### Budget → shared `Rc<RefCell<BudgetFrame>>`

Today the budget lives in five separate thread-locals plus a `Vec<BudgetFrame>`
stack (`builtins.rs:46-54`, push/pop at `598-652`). Restructure so the **active**
frame is a shared cell:

```rust
struct BudgetFrame {
    cost_limit: Option<f64>,
    cost_spent: f64,
    token_limit: Option<u64>,
    tokens_spent: u64,
}
thread_local! {
    // The budget frame in force for the CURRENT TASK. Shared by Rc so that all
    // concurrent tasks spawned inside one `llm/with-budget` charge ONE aggregate.
    static ACTIVE_BUDGET: RefCell<Option<Rc<RefCell<BudgetFrame>>>> = const { RefCell::new(None) };
    static BUDGET_STACK: RefCell<Vec<Option<Rc<RefCell<BudgetFrame>>>>> = const { RefCell::new(Vec::new()) };
}
```

- `llm/with-budget` / `push_budget_scope`: push the current `ACTIVE_BUDGET` onto
  `BUDGET_STACK`, install a **fresh** `Rc<RefCell<BudgetFrame>>` (spent = 0);
  `pop_budget_scope` restores the pushed one. (Semantics preserved: a nested
  `with-budget` resets spend for the inner scope.)
- `track_usage` (`builtins.rs:536-577`) reads/charges `ACTIVE_BUDGET` instead of
  the loose cells; `budget-status` / `stream_budget_pregate` /
  `check_budget_before_dispatch` (the pre-gate at `6819-6842`) all read the active
  frame. Because it is one shared `Rc`, aggregate gating across a fan-out works.
- **Async poller**: capture `let budget_slot = ACTIVE_BUDGET.with(...clone Rc...)`
  at yield time (right beside `usage_accum_slot` at `builtins.rs:6172`), and in the
  poller charge into **that captured frame** rather than the installed one — exactly
  mirroring `usage_accum_slot`. Keep the `USAGE_ACCUM_SUPPRESS`-style guard so the
  charge happens once. A budget overrun still returns `Err` from the poller and
  fails the task (as the sync `?` does today, `builtins.rs:6238-6240`).

`set_budget` / `set_token_budget` / `clear_budget` (the non-scoped public API,
`builtins.rs:599-614`) mutate the active frame (creating one if absent) so the
`llm/set-budget` builtin keeps working.

### The per-task seam (sema-core)

Add a third fn-pointer seam in `crates/sema-core/src/async_signal.rs`, byte-for-byte
mirroring the usage-scope seam (lines 417-476):

- types `LlmScopeCaptureFn` / `LlmScopeTakeFn` / `LlmScopeInstallFn`
- `set_llm_scope_task_callbacks(capture, take, install)`
- `current_llm_scope_boxed()` / `take_task_llm_scope()` / `install_task_llm_scope(ctx)`
- export the six names from `crates/sema-core/src/lib.rs` (mirror lines 19-24).

sema-llm side (mirror `builtins.rs:204-230`): `capture_llm_scope`,
`take_llm_scope`, `install_llm_scope` moving a `Box<dyn Any>` holding `LlmDynScope`;
`register_llm_scope_task_callbacks()` called from `reset_runtime_state`
(`builtins.rs:338`). Snapshot semantics: `capture` clones the read-only values and
clones the budget `Rc` (shared, not deep). `install` mem::replaces the whole
`LlmDynScope` (all thread-locals) and returns the displaced one. `take` mem::takes
it, leaving defaults.

### The scheduler (sema-vm)

Add a fourth per-task field and swap it in lockstep with `otel` / `usage_scope`.
Every edit sits directly beside an existing usage_scope line:

- `Task.llm_scope: Box<dyn Any>` (beside `usage_scope`, `scheduler.rs:73`).
- Seed at spawn: `llm_scope: sema_core::current_llm_scope_boxed()` in **both**
  `Task` constructors (`scheduler.rs:158` and `515`).
- `ReinstallGuard.prev_llm_scope: Option<Box<dyn Any>>` (beside `prev_usage_scope`,
  line 690); restore in `restore_otel` beside the usage-scope block (lines 703-709);
  init to `None` in the panic-test guard (line 1198) and construction (line 1073).
- Install on entry beside the usage-scope install (`scheduler.rs:1066-1067`):
  `let task_llm = std::mem::replace(&mut task.llm_scope, Box::new(()));`
  `let prev_llm_scope = sema_core::install_task_llm_scope(task_llm);`

No new control flow — it rides the existing entry/leave/panic-unwind machinery, so
the isolation + panic-safety guarantees the otel/usage swaps already prove extend
for free.

## Verification (mandatory — AGENTS.md LLM flow)

All keyless, deterministic, via `FakeProvider` (`crates/sema/tests/llm_fake_test.rs`
or `complete_async_test.rs`). **These are the CI regression oracle.**

1. **Restore `async_cache_miss_is_counted`** (Scope A): a single `llm/with-cache`
   wrapping an `async/all` of one spawned `llm/complete` reports `:misses 1` (and a
   same-prompt repeat inside the same scope reports a hit). This is the test deleted
   at `complete_async_test.rs:80-89`; bring it back and make it pass.
2. **Cache flag survives deferral** (Scope A): a fan-out of N spawned completions
   inside one `with-cache` — assert every task saw caching enabled (miss then hit
   counts add up to N, not 0).
3. **Budget gates concurrent fan-out** (Scope B, the correctness fix): script the
   FakeProvider with a known per-call cost; `(llm/with-budget {:max-cost C} (fn ()
   (async/all (map spawn (repeat k complete)))))` where `k * cost > C` must **fail
   with "budget exceeded"** — today it does not. Assert the error, and assert
   `<= ceil(C/cost)` completions actually reached the provider (via `FakeRecorder`).
4. **Nested `with-budget` isolation**: inner scope resets spend, outer scope still
   sees the outer tally after the inner returns (mirror the existing
   `current_usage_accum` scope test at `builtins.rs:7525-7555`).
5. **Tags/metadata attribution** (Scope A): a spawned completion inside a tagged
   scope emits the tags on its span (assert via `sema_otel::testing::install()`, as
   `complete_async_test.rs` already does).
6. **No-regression**: `cargo test -p sema` async + llm suites, plus
   `cargo test --workspace && make examples && make smoke-bytecode && make lint`.
7. **Live smoke** (best-effort, keys in env): one real `with-budget` fan-out on a
   cheap model (`claude-haiku-4-5-20251001` / `gpt-5.4-mini`) confirming the cap
   fires. Ollama-down remains the hard-fail lever for fallback paths.

## Risk & sequencing

- **Scope A first** (cache/tags snapshot via the new seam) — low risk, mirrors
  usage_scope exactly, unlocks tests 1/2/5. Land it, get CI green.
- **Scope B second** (budget → shared `Rc` + poller capture) — higher blast radius
  (`track_usage`, push/pop, pre-gate, poller). Land behind tests 3/4.
- Keep `sema-core` free of any `sema-llm`/`sema-otel` type (type-erased seam only) —
  the whole reason the otel/usage seams exist.
- The single-threaded cooperative model means only one task runs at a time, so the
  shared budget `Rc<RefCell<>>` never races; `RefCell` (not `Mutex`) is correct, as
  it is for `ACTIVE_LEAF_SCOPE`.

## Docs

- Remove the ASYNC-1 entry from `docs/deferred.md` once landed (like the
  LEX-1/VM-1/N7 removals).
- Add ADR #67 (below) with the decision + the shared-frame rationale.
- Update the `workflow :budget` contract note and any `llm/with-budget` doc that
  currently warns concurrent fan-out is not gated.
