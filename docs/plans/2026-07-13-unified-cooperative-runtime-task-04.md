# Task 04: Language Concurrency and Structured Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild Sema promises, channels, observational waits, detached spawn,
owned concurrency, cancellation, timeout, and root barriers on the unified
runtime contracts.

**Architecture:** Promise-taking APIs register observations and never acquire
ownership. Thunk-taking APIs create a scope that owns direct children and must
cancel and reap them before returning after failure, cancellation, race loss, or
timeout. All APIs suspend through `NativeOutcome`; no builtin runs a nested
scheduler or encodes a wait by returning dummy `nil`.

**Tech Stack:** Rust, `sema-core`, `sema-vm`, `sema-stdlib`, Sema prelude macros,
deterministic async integration tests.

## Execution contract

- **Status:** Ready only after Task 03 is accepted and committed.
- **Dependencies:** Interpreter-owned multi-root runtime, VM continuations,
  fair bounded driving, generations, cleanup registry, captured-cell coherence,
  and the temporary one-way `LegacyAsyncAbiAdapter` that carries current
  language behavior without owning or driving a scheduler.
- **Immutable inputs:** Master promise states, observational operations, detached
  spawn, owned operations, `async/run`, validation, and structured conditions.
- **Exact start state:** Clean worktree; latest commit subject is
  `refactor(runtime): install interpreter-owned scheduler`; only Task 04 RED
  cases listed in Task 03 evidence remain.
- **Parallel work:** Promise/observation tests and owned-scope tests may begin in
  parallel. One integration owner controls runtime wait/scope/task files and
  public registration. Channel work begins after observation registration is
  stable. Call-site migration begins after final API behavior is GREEN.

## Global constraints

- Tasks 01–03 must be accepted. Preserve their exact fairness, stale-delivery,
  captured-cell, and multiple-root oracles.
- `async/all`, `async/race`, and `async/timeout` never cancel supplied promises.
- `async/spawn-all`, `async/map`, `async/pool-map`, `async/race-owned`, and
  `async/with-timeout` own only the child tasks they create.
- `async/spawn` creates an interpreter-owned detached task. Normal root
  settlement does not cancel it; origin-root cancellation does.
- Cancellation is a distinct settlement and a structured catchable condition.
- Settlement sequence, not list order or hash iteration, decides races.
- Cleanup is part of completion. Owned APIs may not return while owned children
  remain live, except quarantined-bounded operations transferred to the cleanup
  registry with a proven deadline.
- No API accepts an implicit unbounded resource, no infinite in-process test is
  allowed, and no profiling occurs here.

---

## Files and responsibilities

**Create**

- `crates/sema-vm/src/runtime/scope.rs` — owned child scopes and cleanup state.
- `crates/sema-stdlib/src/async_owned.rs` — thunk-taking structured operations.
- `crates/sema/tests/async_contract_test.rs` — language contract matrix.
- `crates/sema/tests/async_owned_test.rs` — ownership/reaping matrix.
- `crates/sema/tests/async_condition_test.rs` — catch/rethrow/trace behavior.
- `docs/plans/evidence/unified-cooperative-runtime/task-04.md` — exact API and
  cleanup evidence.
- `docs/plans/reviews/unified-cooperative-runtime/task-04.md` — independent
  review report.

**Modify**

- `crates/sema-vm/src/runtime/promise.rs` — complete public promise predicates,
  diagnostics, and observation semantics on Task 03's four-state registry.
- `crates/sema-vm/src/runtime/channel.rs` — complete bounded-channel validation,
  close behavior, and public waits on Task 03's identity registry.
- `crates/sema-core/src/value.rs` — four-state promise handle and channel handle.
- `crates/sema-core/src/cycle.rs` — trace pending observations, settlements, and
  channel values.
- `crates/sema-core/src/error.rs` — language condition conversion and predicates.
- `crates/sema-vm/src/runtime/{mod.rs,task.rs,wait.rs,drive.rs,cleanup.rs}` —
  promise/scope/channel events, origin barriers, and explicit cancellation.
- `crates/sema-stdlib/src/async_ops.rs` — detached and observational primitives.
- `crates/sema-stdlib/src/lib.rs` — register owned primitives.
- `crates/sema-eval/src/prelude.rs` — define public macros without rebuilding
  ownership from observational operations.
- `crates/sema/tests/vm_async_test.rs` — remove superseded cancellation oracles
  and retain compatibility aliases.
- `crates/sema/tests/embed_timeout_reap_test.rs` — migrate cancellation-required
  scenarios to `async/with-timeout`.
- Every integration test using `async/timeout` as a cancellation guard — retain
  `async/timeout` only when continued background work is intended; otherwise use
  `async/with-timeout` with a thunk.
- `docs/internals/async-runtime-inventory.md` and the legacy baseline — remove
  language-layer bridges and record any compatibility aliases.

`LegacyAsyncAbiAdapter` is a migration input, not a permanent compatibility
surface. Tasks 1–7 replace each adapted promise, spawn, observation, timer,
barrier, and channel path with runtime-native state. Task 7 deletes the adapter
and every remaining signal/resume TLS or scheduler-drive symbol before Task 8
begins, except the exact `AwaitIo` signal functions and producer-side
`take_resume_value` references inventoried under `LegacyAwaitIoBridge` for Tasks
05–08. The separate `LegacyRuntimeBridge` may remain only for producers with an
explicit Task 05–08 owner; neither bridge provides scheduling or runtime
driving.

## Exact language surface

| Form | Input ownership | Required terminal behavior |
| --- | --- | --- |
| `(async/spawn thunk)` | creates detached task | return promise immediately |
| `(async/resolved value)` | creates synthetic promise | allocate settlement sequence and return a returned promise |
| `(async/rejected error)` | creates synthetic promise | allocate settlement sequence and return a failed promise |
| `(async/await promise)` | observes | preserve returned/failed/cancelled outcome |
| `(await promise)` | compatibility alias | identical to `async/await` |
| `(async/all promises)` | observes | values in input order; first sequenced failure/cancellation; others continue |
| `(async/race promises)` | observes | lowest settlement sequence wins; losers continue |
| `(async/timeout ms promise)` | observes | timeout ends this wait only; producer continues |
| `(async/cancel promise)` | explicitly cancels target | idempotent boolean indicating newly requested cancellation |
| `(async/sleep ms)` | owns timer wait | return `nil` after validated duration |
| `(async/run)` | observes origin barrier | wait for other tasks of current origin root, return `nil` |
| `(async/spawn-all thunks)` | owns created children | ordered values or fail-fast cancel/reap |
| `(async/map f items)` | owns created children | unbounded fan-out, ordered values, fail-fast cleanup |
| `(async/pool-map f items n)` | owns created children | at most `n` active, ordered values, fail-fast cleanup |
| `(async/race-owned thunks)` | owns created children | preserve winner, cancel/reap losers |
| `(async/with-timeout ms thunk)` | owns one child | preserve child or timeout after cancel/reap |

Do not add a promise-taking `with-timeout` overload or thunk-taking `timeout`
overload. Arity/type errors explain the distinction and name the other API.

Promise predicates partition state:

```text
async/promise?   true for all promise handles
async/pending?   true only before settlement
async/resolved?  true only for Returned
async/rejected?  true only for Failed
async/cancelled? true only for Cancelled
```

The final predicate surface is `async/promise?`, `async/pending?`,
`async/resolved?`, `async/rejected?`, `async/cancelled?`, and `async/forced?`.
`async/forced?` is true for any terminal state and does not collapse the
partition. Remove any other migration-only predicate alias in Task 08. A failed
promise whose message contains “cancelled” is still failed.

## Exact runtime structures

```rust
pub enum PromisePoll {
    Pending,
    Settled(Rc<TaskSettlement>),
}

pub struct Observation {
    pub observer: TaskId,
    pub promise: PromiseId,
    pub wait: WaitId,
}

pub struct OwnedScope {
    pub id: ScopeId,
    pub owner: TaskId,
    pub children: Vec<TaskId>,
    pub primary: Option<Rc<TaskSettlement>>,
    pub state: ScopeState,
}

pub enum ScopeState {
    Running,
    Cancelling,
    Reaping,
    Complete,
}
```

An observation deregisters on observer cancellation or timeout. It never calls
producer cancellation. An owned scope records the primary settlement before
starting cleanup; cleanup errors become suppressed diagnostics and never replace
that primary outcome.

## Task 1: Implement four-state promises and observations

**Files:** `runtime/promise.rs`, `value.rs`, `cycle.rs`,
`async_contract_test.rs`

- [ ] **Step 1: Write failing promise-state tests**

Cover all four states, repeated polling, multiple observers, duplicate promise
entries, final-handle drop while pending, unobserved failure diagnostics, and GC
of resolved and unresolved promise graphs. Create synthetic returned/failed
promises in reverse observation order and assert their creation-time
`SettlementSeq`; `async/cancel` returns `#f` for both.

- [ ] **Step 2: Implement runtime-owned settlement storage**

The handle references a stable promise identity; the runtime stores task and
settlement state. A settlement stores one `SettlementSeq` and wakes registered
observers in registration order.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test async_contract_test -- promise
cargo test -p sema-core cycle
```

Expected: promise partition and GC tests pass.

## Task 2: Implement detached spawn, await, cancellation, and sleep

**Files:** `async_ops.rs`, runtime task/wait/timer files,
`async_contract_test.rs`

> **PROGRESS (2026-07-14) — `async/sleep` is GREEN end-to-end through the unified
> runtime.** The first async op runs as a real root through `Runtime`:
> `(async/sleep ms)` (and `(begin (async/sleep ms) …)`) evaluate via
> `Interpreter::eval_str_via_runtime` and settle only after the runtime's own
> timer fires. Gate tests (un-ignored) in `sema-eval` `mod runtime_eval_tests`:
> `eval_via_runtime_async_sleep_settles_after_timer_fires` and
> `eval_via_runtime_async_sleep_resumes_and_continues`.
>
> **VM suspend seam used.** A native yields from inside a `run_quantum` via the
> existing TLS yield signal, which the VM already surfaces as
> `VmExecResult::AsyncYield(YieldReason)` (vm.rs native-dispatch arms; the frame
> is parked with pc past the call and a nil placeholder on the stack top). The
> runtime's `visit_ready` VM-quantum arm (state.rs) now handles
> `AsyncYield(Sleep(ms))`: it keeps the VM parked in `vm_call`, and a new
> `TaskAction::VmSleep` arms a runtime timer (`TimerQueue` + `issue_internal_wait`
> + `TaskRecord::wait`). When `fire_timer` wakes the task, the same VM frame
> resumes in place (the nil placeholder is the resume value). This is the
> **VM-resumes-itself** model (mirrors the legacy scheduler's
> `replace_stack_top` + re-run), which is the correct shape for a mid-VM
> suspension — unlike the native-continuation `NativeOutcome::Suspend` path,
> whose `Box<dyn NativeContinuation>` would settle the root early rather than
> continue the program.
>
> A ctx-less yielding native detects the runtime via a new thread-local
> `IN_RUNTIME_QUANTUM` (sema-core `async_signal.rs`), set by
> `RuntimeQuantumGuard`; `async/sleep` yields when
> `in_async_context() || in_runtime_quantum()`. Legacy behavior is unchanged
> (the flag is false off-runtime; `vm_async_test` shows 0 new failures).
> `eval_str_via_runtime`'s drive loop now waits out `DriveState::Idle`
> deadlines on the real clock before re-driving.
>
> **REMAINING for full Task 2 through the runtime (ordered sub-slices):**
> 1. `async/sleep` duration-validation RED cases (`sleep_rejects_*`) — reject
>    negative-before-rounding etc.; gate: `vm_async_test -- sleep_rejects`.
> 3. `async/cancel` through the cancellation-parent graph. **(DONE 2026-07-15:
>    try/catch *around* an `await` of a rejected spawned promise is now catchable
>    regardless of scheduling order — see the rejected-await note below.)**
> The `NativeOutcome::Suspend(WaitKind::{Promise,PromiseSet,Channel,Timer})` path
> in `apply_native_outcome` is for natively-implemented suspending ops (async/all
> etc.); `Timer` there is still routed to the "wait protocol not active" error and
> is a separate future slice from VM-level sleep.

> **PROGRESS (2026-07-15) — `async/spawn` + `async/await` are GREEN end-to-end
> through the unified runtime.** A detached task spawned via `(async/spawn thunk)`
> runs as a runtime-owned VM task, settles its own Sema `AsyncPromise`, and
> `(await promise)` parks the awaiting frame until it settles — all through
> `Interpreter::eval_str_via_runtime`. Gate tests (un-ignored) in `sema-eval`
> `mod runtime_eval_tests`: `eval_via_runtime_await_spawn_returns_value`,
> `eval_via_runtime_await_two_spawned_tasks` (concurrent detached tasks), and
> `eval_via_runtime_await_spawn_that_sleeps` (the detached task itself parks on a
> timer and resumes). The prior `..._async_spawn_is_unsupported_boundary` ignore
> is gone.
>
> **Seam (mirrors the `async/sleep` VM-resumes-itself template).** Two new
> `YieldReason`s surface from inside a `run_quantum`: `Spawn(thunk)` (new variant
> in sema-core `async_signal.rs`) and the existing `AwaitPromise(promise)`. Both
> `async/spawn` and `async/await` (sema-stdlib `async_ops.rs`) yield when
> `in_runtime_quantum()`. The runtime's `visit_ready` AsyncYield arm (sema-vm
> `runtime/state.rs`) maps them to `TaskAction::VmSpawn`/`VmAwait`:
> - `VmSpawn` (`spawn_detached`): builds a task VM from the thunk closure
>   (`extract_vm_closure` + `setup_for_call`, `close_closure_upvalues_for_foreign_run`),
>   allocates a Pending `AsyncPromise` (registered as a GC candidate), inserts a
>   detached origin-root child task (`spawned_promises` map — NOT the root's main
>   task), enqueues it Ready, and resumes the spawner with the promise value via
>   `replace_stack_top` (`resume_running_vm` stamps `RuntimeTask.vm_resume`).
> - `VmAwait` (`await_promise`): if the promise already settled, resumes in place;
>   else parks the frame on an `issue_internal_wait` key tracked in `promise_waits`.
> - A detached task's completion routes through `settle_task` → `settle_spawned`
>   (fills the Sema promise state, wakes every awaiter with the value or the
>   rejection/cancellation), instead of settling a root. Await failures are
>   applied as `VmResume::Fail`, which now RAISES the error inside the parked
>   frame (see the rejected-await note below) rather than settling the task.
> No new drive-loop work source is needed: a pending promise always has a
> live Ready/Waiting settler task (or is already settled), so the existing
> timer-idle handling in `eval_str_via_runtime`'s drive loop covers gate 3.
>
> **REJECTED-AWAIT IS UNIFORMLY CATCHABLE (2026-07-15).** A rejected `await` now
> raises an ORDINARY catchable Sema error inside the parked frame, regardless of
> whether the awaited promise was still Pending or already-settled when `await`
> ran. Previously the `VmResume::Fail` arm in `visit_ready` (`runtime/state.rs`)
> dropped the parked `vm_call` and settled the task Failed, tearing down the VM
> stack WITHOUT running its exception machinery — so a `try`/`catch` around the
> `await` was bypassed only when the promise happened to still be pending (the
> already-settled case hit the native's `Rejected` fast path in `async_ops.rs`
> and returned a normal VM error). The fix: `VM::resume_with_error` (sema-vm
> `vm.rs`) arms a `pending_resume_error`; the next `run_inner` discards the parked
> nil placeholder and calls `handle_exception` at the parked call site — the exact
> behavior of the native-`Err` path (`handle_err!`). The `VmResume::Fail` arm now
> re-runs the parked frame via the shared `run_parked_quantum` helper with the
> error armed, instead of settling directly. Handled → the frame resumes in its
> `catch`; uncaught → the error surfaces as `Err` out of `run_quantum` and the
> normal `TaskAction::VmResult(Err)` path settles the task Failed (uncaught
> behavior unchanged). Gate: `runtime_await_pending_rejection_is_catchable` in
> `sema-eval` `mod runtime_eval_tests` (asserts both catchable and uncaught-Failed).

> **PROGRESS (2026-07-15) — channels are GREEN end-to-end through the unified
> runtime, backed by the canonical `ChannelRegistry`.** `channel/new`,
> `channel/send`, `channel/recv`, and `channel/close` run through
> `Interpreter::eval_str_via_runtime` with cross-task rendezvous, buffered FIFO,
> blocking send/recv, and close. Sema API names used: `channel/new` (capacity),
> `channel/send`, `channel/recv`, `channel/close` (+ the closed sentinel: recv
> from closed+empty returns `nil`; send-to-closed raises a catchable condition).
> Gate tests (un-ignored) in `sema-eval` `mod runtime_eval_tests`:
> `runtime_channel_rendezvous_across_tasks`, `runtime_channel_buffered_fifo_order`,
> `runtime_channel_blocking_send_parks_until_received`,
> `runtime_channel_blocking_recv_parks_until_sent`,
> `runtime_channel_recv_after_close_drains_then_sentinel`,
> `runtime_channel_send_to_closed_errors`, `runtime_channel_rejects_invalid_capacity`.
>
> **Seam (mirrors the `async/spawn`/`async/await` VM-resumes-itself template).**
> Three `YieldReason`s surface from inside a `run_quantum`: the existing
> `ChannelSend(ch, value)` / `ChannelRecv(ch)` and a new `ChannelClose(ch)`
> (sema-core `async_signal.rs`). The channel natives (sema-stdlib `async_ops.rs`)
> yield these when `in_runtime_quantum()` instead of touching the Sema `Channel`
> buffer — so the runtime `ChannelRegistry` is the SINGLE source of truth for
> buffering + rendezvous in-runtime. `channel/send`'s eager `ch.closed` check and
> `channel/close`'s `ch.closed.set(true)` still run synchronously so a
> send-to-closed keeps the legacy "value … was dropped" message without a yield.
> `visit_ready`'s AsyncYield arm (sema-vm `runtime/state.rs`) maps them to
> `TaskAction::VmChannelSend`/`VmChannelRecv`/`VmChannelClose`, handled by
> `channel_send`/`channel_receive`/`channel_close`:
> - The Sema channel `Value` carries no `ChannelId`, so `resolve_channel` bridges
>   `Rc<Channel>` pointer-identity → a runtime `ChannelId` (`channel_bridge` map,
>   allocated lazily with the Sema channel's capacity on first op; the `Rc` clone
>   pins the address). This is the smallest bridge — no new channel store; the
>   canonical `ChannelRegistry`/`ChannelResult` back everything.
> - Immediate results resume the frame in place (`resume_running_vm`): `Sent`→nil,
>   `Received(v)`→v, `Closed`→nil (recv sentinel) or a closed-send error.
> - A full-send / empty-recv parks on an `issue_internal_wait` key tracked in
>   `channel_waits`; a counterpart's `ChannelWake` (drained via `pop_wake` after
>   every send/recv/close) resumes it. `consume_channel_wake` now routes VM-quantum
>   waiters (`consume_vm_channel_wake`) before the continuation-model protocol path.
> Capacity validation (`channel/new`, zero/negative → condition) already runs in
> the native before any allocation, so it surfaces as `Err` with no runtime change.
>
> **FOLLOW-UP (2026-07-15) — observational channel ops now also read the registry
> (found by adversarial verification).** The non-blocking observers
> (`channel/count`, `channel/empty?`, `channel/full?`, `channel/try-recv`) still
> read the Sema `Channel` buffer, which is empty under the unified runtime — so
> `channel/count` reported 0 and `channel/try-recv` returned nil while stranding
> the sent value in the registry (silent data loss). Fix mirrors the send/recv
> seam but is SYNCHRONOUS (no park): two new `YieldReason`s —
> `ChannelInspect(ch, ChannelQuery)` and `ChannelTryRecv(ch)` — map to
> `TaskAction::VmChannelInspect`/`VmChannelTryRecv`, handled by `channel_inspect`
> (registry `inspect`) and `channel_try_receive` (registry `try_receive`, draining
> any wake it queues for an unblocked sender). Both resolve the channel via the
> existing `resolve_channel` bridge and resume the frame in place with
> `resume_running_vm` — NO `issue_internal_wait`, NO `channel_waits` entry. Legacy
> (non-quantum) paths are unchanged. `channel/closed?` is already correct: its
> `ch.closed` flag is set synchronously by `channel/close` in both paths, so it
> stays on the Sema struct. New gates (un-ignored) in `sema-eval`
> `mod runtime_eval_tests`: `runtime_channel_count_reflects_buffered_sends`,
> `runtime_channel_try_recv_returns_buffered_value`,
> `runtime_channel_empty_and_full_reflect_registry_state`,
> `runtime_channel_try_recv_after_close_drains_then_sentinel`.

- [ ] **Step 1: Write failing detached-lifetime tests**

Assert a detached task can outlive normal root settlement and be awaited from a
later root; origin-root cancellation cancels descendants; cancelling one waiter
does not cancel the producer; explicit promise cancellation is idempotent; and
shutdown cancels the remaining detached task. Pass a promise from root A to root
B and prove B's explicit `async/cancel` can cancel that exact task while B's root
cancellation cannot reach unrelated A tasks.

- [ ] **Step 2: Implement through `NativeOutcome`**

Spawn creates `LifetimeOwner::Interpreter`. Await registers one observation.
Sleep registers an interruptible timer. Cancellation propagates through the
cancellation-parent graph, not observer edges.

- [ ] **Step 3: Test duration validation**

Use zero, sub-millisecond, negative, NaN, positive/negative infinity, maximum,
maximum plus one, and conversion overflow. Native/WASM rounding policy must be
one shared function.

- [ ] **Step 4: Run**

```bash
cargo test -p sema-lang --test async_contract_test -- detached
cargo test -p sema-lang --test async_contract_test -- duration
```

Expected: all selected tests pass without wall-clock sleeps.

> **PROGRESS (2026-07-15) — `async/cancel` is GREEN end-to-end through the
> unified runtime (`eval_str_via_runtime`).** Wired on the same
> spawned-`Rc<AsyncPromise>` seam as `async/spawn`/`async/await`.
> - New `YieldReason::Cancel(Rc<AsyncPromise>)` (`sema-core/async_signal.rs`).
>   `async/cancel` (sema-stdlib `async_ops.rs`) yields it when
>   `in_runtime_quantum()` instead of driving the legacy cancel callback; the
>   legacy scheduler gains an exhaustive `Cancel` arm (unreachable in-runtime).
> - The VM surfaces it as `AsyncYield`; `visit_ready` maps it to
>   `TaskAction::VmCancel`; `cancel_promise` (sema-vm `runtime/state.rs`)
>   resolves promise → runtime `TaskId` (via the promise's `task_id` cell) and
>   calls `TaskRecord::request_cancellation(CancelReason::Explicit)`, returning
>   `#t` ONLY for the FIRST request of a still-pending spawned task; `#f` for a
>   synthetic promise (`task_id == 0`), an already-terminal promise, an
>   already-requested task, or a reaped task. Idempotent. The requester frame
>   resumes with the boolean via `replace_stack_top`.
> - No new interruption code was needed: the drive loop's existing
>   `cancel_waiting` pass (source 2, run every drive turn — not just at shutdown)
>   already deregisters a cancelled task's active wait. A task blocked on a long
>   `async/sleep` has its far-future timer CANCELLED and is woken, then
>   `visit_ready`'s cancellation arm settles it `Cancelled` via
>   `settle_task`→`settle_spawned` — so it stops PROMPTLY at the next cooperative
>   boundary, never after the full sleep. A Ready (not-yet-parked) cancelled child
>   is settled directly by `visit_ready`.
> - **Awaiting a cancelled promise raises a STRUCTURED catchable `:cancelled`
>   condition.** `await_cancelled_error` (state.rs) and `cancelled_error`
>   (async_ops.rs) now return `SemaError::cancelled_condition(..)` (a
>   `SemaError::Condition` map with `:type :cancelled`), so a `(catch e …)` binds
>   the condition map and `(:type e)` is `:cancelled`. The Sema `PromiseState::
>   Cancelled` variant carries no `CancelReason`, so a generic `Explicit` reason
>   is used (NOTE: to surface the real root/owner/timeout reason on the condition,
>   `PromiseState::Cancelled` would need to carry the reason — deferred).
> - Un-ignored gate tests in `sema-eval` `mod runtime_eval_tests`:
>   `runtime_async_cancel_first_request_true_second_false` (gate 1),
>   `runtime_async_cancel_synthetic_promise_is_false` (gate 1b),
>   `runtime_await_cancelled_promise_raises_cancelled_condition` (gate 2,
>   `(:type e)` → `:cancelled`), `runtime_await_cancelled_uncaught_settles_errored`
>   (gate 2b), `runtime_cancel_sleeping_task_stops_promptly` (gate 3, wall-clock
>   bounded well under the 100s sleep).

## Task 3: Implement observational `all`, `race`, and `timeout`

**Files:** `async_ops.rs`, `runtime/promise.rs`, `runtime/wait.rs`,
`vm_async_test.rs`, `async_contract_test.rs`

> **PROGRESS (2026-07-15) — `async/all`, `async/race`, `async/timeout` are GREEN
> end-to-end through the unified runtime (`eval_str_via_runtime`).** Wired on the
> spawned `Rc<AsyncPromise>` seam (the same seam `async/spawn`/`async/await` use),
> NOT the `PromiseId` `promise_set_response` path — that registry has no entries
> for VM-spawned promises and explicitly rejects `Timeout`
> (`install_promise_wait`). Design:
> - New `YieldReason::AwaitPromiseSet { promises, mode }` +
>   `sema_core::PromiseSetKind::{All,Race,Timeout(ms)}` (`async_signal.rs`). Each
>   combinator native, when `in_runtime_quantum()`, maps its Sema promise args and
>   yields this instead of driving the legacy scheduler (`async_ops.rs`).
> - The VM surfaces it as `AsyncYield`; `run_parked_quantum` maps it to
>   `TaskAction::VmAwaitSet`; `await_promise_set` parks the frame in a new
>   `promise_set_waits` map (and, for `Timeout`, arms a deadline timer on the same
>   wait key). `settle_spawned` calls `wake_promise_set_waiters` on every settle;
>   `evaluate_promise_set` computes the input-order `All` list / fail-fast /
>   first-settled `Race` winner and resumes via `replace_stack_top` (value) or
>   `resume_with_error` (failure/cancel/timeout). `fire_timer` delivers the
>   `:timeout` error when the deadline key belongs to a set-wait. Supplied
>   promises are only OBSERVED — never cancelled (verified: siblings/losers/
>   producers still settle and are awaitable afterward).
> - Un-ignored gate tests in `sema-eval` `mod runtime_eval_tests`:
>   `runtime_async_all_returns_values_in_input_order`,
>   `runtime_async_all_empty_input_is_empty_list`,
>   `runtime_async_all_failure_does_not_cancel_sibling`,
>   `runtime_async_race_returns_fast_and_loser_continues`,
>   `runtime_async_timeout_settled_wins`,
>   `runtime_async_timeout_pending_raises_and_producer_continues`.
> - NOT YET DONE (legacy-path RED, unchanged): the `vm_async_test` oracles in
>   Step 3 (`async_all_failure_does_not_cancel_supplied_sibling`,
>   `async_race_does_not_cancel_supplied_loser`) run through the LEGACY scheduler
>   via `eval`, which still cancels siblings on short-circuit. Turning those GREEN
>   requires routing the legacy `async`/`eval` path (or the whole test harness)
>   through the runtime — out of scope for this observational slice.

- [ ] **Step 1: Write exact failing observation tests**

Include empty all, ordered all results, fail/cancel short-circuit with surviving
siblings, empty race error, pending race, already-settled race in reverse input
order, return/error/cancel winners, duplicate handles, zero timeout with already
settled promise, timeout with a producer that later completes, and waiter
cancellation with a producer that later completes.

- [ ] **Step 2: Implement observation sets**

For pre-settled inputs choose the lowest stored `SettlementSeq`. For future
settlements wake on the first sequence assigned by the runtime. Remove all other
observation registrations when the waiter finishes; do not cancel producers.

- [ ] **Step 3: Turn the Task 01 observation oracles GREEN**

Preserve these corrected Task 01 tests without weakening their expected values:

- `race_with_settled_winner_does_not_cancel_supplied_loser`;
- `async_race_does_not_cancel_supplied_loser`;
- `async_all_failure_does_not_cancel_supplied_sibling`.

They prove supplied work continues and can still be awaited. The obsolete
implicit-cancellation names were already removed in Task 01 and must not be
reintroduced.

- [ ] **Step 4: Run**

```bash
cargo test -p sema-lang --test async_contract_test -- observational
cargo test -p sema-lang --test vm_async_test -- async_all
cargo test -p sema-lang --test vm_async_test -- async_race
cargo test -p sema-lang --test vm_async_test -- async_timeout
```

Expected: all selected tests pass with no implicit target cancellation.

## Task 4: Implement owned scopes and cleanup

**Files:** `runtime/scope.rs`, `runtime/cleanup.rs`, `async_owned.rs`,
`async_owned_test.rs`

- [ ] **Step 1: Write failing scope transition tests**

Cover successful completion, one failure with pending siblings, simultaneous
failures ordered by sequence, parent cancellation, cancellation-hook failure,
late external completion during reaping, and transfer of a quarantined-bounded
operation. Assert zero live owned children after API settlement.

- [ ] **Step 2: Implement scope cleanup state machine**

The owner creates children with both cancellation parent and lifetime owner set
to the scope. On primary settlement: record it, cancel unfinished children,
drain interruptible cleanup, transfer allowed quarantine entries, reap tasks,
then resume the owner with the preserved outcome.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test async_owned_test -- scope
cargo test -p sema-vm runtime::tests::cleanup
```

Expected: transition and zero-leak assertions pass.

## Task 5: Implement every thunk-taking API

**Files:** `async_owned.rs`, `prelude.rs`, `async_owned_test.rs`

- [ ] **Step 1: Add table-driven API tests**

For each owned API test empty input where allowed, one item, ordered success,
failure, cancellation, parent cancellation, captured lexical mutation, context
inheritance, and cleanup counts. Additionally:

- `race-owned`: empty is error; returned/failed/cancelled winner; pre-settlement
  sequence preserved before loser cleanup;
- `with-timeout`: child wins at equal recorded sequence; deadline wins pending
  child; child cancellation remains cancellation rather than timeout;
- `pool-map`: `n = 1`, `n > item count`, `n <= 0`, large invalid integer, and
  never more than `min(n, item_count)` active tasks.

- [ ] **Step 2: Implement primitives, then thin public macros**

Prelude macros may package thunks and arguments only. They must not express
ownership as `(async/all (map async/spawn ...))`, because that loses the scope.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test async_owned_test
cargo test -p sema-lang --test vm_async_test -- pool_map
```

Expected: all owned-operation cases pass and active-task high-water assertions
match the requested bound.

> **Progress (2026-07-15, owned-combinator slice — DONE through `eval_str_via_runtime`):**
> The five thunk-taking combinators are wired through the unified runtime with
> un-ignored gates in `sema-eval` `mod runtime_eval_tests` (14 new, all green):
> - `async/spawn-all thunks` — input-order values; empty → `()`; a failing child
>   CANCELS the still-running sibling before its side effect
>   (`runtime_owned_spawn_all_failure_cancels_sibling`, flag stays 0 — the OWNED
>   dual of the observational `runtime_async_all_failure_does_not_cancel_sibling`).
> - `async/map f items` — one owned child per item, input-order, same fail-fast.
> - `async/pool-map f items n` — at most `n` workers active (shared max-counter
>   proves exactly 2 for n=2/6 items), input-order, `n <= 0` is an arg error,
>   fail-fast cancels the pending sibling.
> - `async/race-owned thunks` — ≥1 required; first settlement wins; losers are
>   cancelled before their side effects; a failing winner re-raises.
> - `async/with-timeout ms thunk` — deadline cancels the slow child (structured
>   `:timeout`); a fast child's value/error is preserved.
>
> Ownership is realized by COMPOSITION over the observational combinators plus an
> explicit cancel-and-reap, NOT a full Rust `runtime/scope.rs` state machine:
> `__owned-all` = `(try (async/all children) (catch e (__cancel-all children)
> (throw e)))`; race-owned/with-timeout wrap `async/race` the same way. This
> keeps the combinators working on BOTH the top-level scheduler and the runtime
> (pool_map_test + vm_async_test unchanged). Children are spawned at bytecode
> level (`__spawn-thunks`/`__spawn-apply`), never `(map async/spawn …)` — that
> yields "async yield outside of scheduler context". Full zero-leak reaping,
> simultaneous-failure sequencing, and quarantine transfer (Task 4's Rust scope
> machine) remain a later slice.
>
> Two runtime/dispatch bugs blocked this and were fixed (`crates/sema-vm/src/vm.rs`,
> `crates/sema-eval/src/eval.rs`):
> 1. `collect_native_names` classified prelude functions (VM closures *wrapped* in
>    a `NativeFn`) as "known natives", so the compiler emitted a native-call that
>    ran them through the wrapper's synchronous nested path — suspending the
>    quantum and breaking any spawn/await/channel yield inside. Now excludes VM
>    closures so they dispatch in-VM.
> 2. `run_inner` captured `base_functions` from the *current* `self.functions` at
>    entry. When a quantum yielded mid-call (e.g. `channel/send` inside a prelude
>    helper), the next quantum adopted the callee's table as the main's, so a
>    later `MakeClosure` indexed a too-short table (out-of-bounds). Added a stable
>    `VM::base_functions` field set at construction. Also snapshot a spawned
>    closure's open upvalues against the (parked) spawning VM in `spawn_detached`
>    (`close_closure_upvalues_with_owner`) — the native-call guard is gone by the
>    time the runtime services the Spawn yield.

## Task 6: Implement origin-root `async/run`

**Files:** runtime task/scope files, `async_ops.rs`, `async_contract_test.rs`

- [ ] **Step 1: Write failing barrier tests**

Test zero other tasks, direct detached tasks, transitively spawned descendants,
already-settled tasks, unobserved failure, unrelated root work, a descendant
spawned while the barrier is pending, and cancellation of the barrier waiter.

- [ ] **Step 2: Implement a generation-aware origin barrier**

The barrier settles only when the origin root has no other pending task. It does
not own those tasks and never starts a nested drive loop. A task spawned with the
same origin before quiescence extends the barrier.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test async_contract_test -- async_run
```

Expected: exact barrier tests pass; unrelated root settlement is not required.

## Task 7: Implement channels on runtime waits

**Files:** `runtime/channel.rs`, `value.rs`, `cycle.rs`, `async_ops.rs`,
`async_contract_test.rs`

- [ ] **Step 1: Write failing channel matrix**

Cover FIFO send/recv, close, blocked sender/receiver cancellation, close with
waiters, task failure while blocked, value tracing, multiple roots, capacity
zero/negative/overflow/allocation-impossible, and no lost wakeup at every
enqueue/dequeue boundary.

- [ ] **Step 2: Implement channel wait registration**

Channel state owns buffered `Value`s and waiter IDs on the runtime thread.
Cancellation removes exactly one wait generation. Capacity validation happens
before integer conversion and allocation.

- [ ] **Step 3: Delete the temporary language ABI adapter**

After channel tests are GREEN, remove `LegacyAsyncAbiAdapter` and all remaining
production references to `set_yield_signal`, `take_yield_signal`,
`set_resume_value`, `take_resume_value`, `call_run_scheduler*`,
`SchedulerTarget`, and `SchedulerRunResult`. Refresh the inventory and legacy
baseline. The only permitted matches are the exact `LegacyAwaitIoBridge`
signal/poller and producer-side compatibility checks assigned to Tasks 05–08.
No replacement may own a task store, timer, ready queue, clock, or nested drive
loop.

- [ ] **Step 4: Run**

```bash
cargo test -p sema-lang --test async_contract_test -- channel
cargo test -p sema-core cycle
rg -n 'LegacyAsyncAbiAdapter|set_yield_signal|take_yield_signal|set_resume_value|take_resume_value|call_run_scheduler|SchedulerTarget|SchedulerRunResult' crates --glob '*.rs'
```

Expected: channel matrix passes without polling or wall-clock sleeps; the
legacy-symbol search has no production language-layer matches outside the exact
inventoried `LegacyAwaitIoBridge` producer list.

## Task 8: Migrate cancellation-dependent call sites

**Files:** `embed_timeout_reap_test.rs` and every test/source match from:

```bash
rg -n 'async/timeout' crates examples playground website docs
```

- [ ] **Step 1: Classify every match**

Record `observation-intended`, `ownership-required`, or `documentation` in Task
04 evidence. For ownership-required cases change promise construction plus
timeout to `(async/with-timeout ms (fn () ...))`.

- [ ] **Step 2: Add producer-survival assertions to observational uses**

A retained `async/timeout` test must either later await the producer or observe
a durable side effect proving it continued.

- [ ] **Step 3: Run every affected integration test target**

Record exact target commands and outcomes in evidence; do not substitute one
workspace test command for the per-target attribution.

## Task 9: Structured conditions, traces, verification, and review

- [ ] **Step 1: Test sticky cancellation cleanup, catch/rethrow, and ancestry**

Cancel at a waiting and CPU-quantum boundary. A handler may inspect the
structured condition and run synchronous bounded cleanup, but an attempted
suspension immediately observes the same cancellation, returning from the
handler still settles `Cancelled`, and interpreter shutdown cannot be caught as
successful completion. Assert the promise stays pending-with-cancellation-
requested until wait/resource cleanup finishes, then becomes terminally
cancelled.

```bash
cargo test -p sema-lang --test async_condition_test
```

Expected: cancellation maps contain the accepted stable `:type`, `:reason`, and
`:root-id` keys, with `:scope-id`/`:operation-id` when that relation exists;
timeout maps contain `:type` and `:duration-ms`; rethrow preserves
identity and `spawned by`/`awaited by`/`cancelled by` links.

- [ ] **Step 2: Run layer gates**

```bash
cargo test -p sema-core
cargo test -p sema-vm
cargo test -p sema-eval
cargo test -p sema-lang --test vm_async_test
cargo test -p sema-lang --test async_contract_test
cargo test -p sema-lang --test async_owned_test
cargo test -p sema-lang --test async_condition_test
cargo test -p sema-lang --test unified_runtime_watchdog_test
cargo test -p sema-lang --test runtime_conformance_test
cargo fmt --all -- --check
cargo clippy -p sema-core -p sema-vm -p sema-eval -p sema-stdlib \
  --all-targets -- -D warnings
scripts/check-unified-runtime-legacy.sh > /tmp/runtime-legacy.actual
diff -u docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline /tmp/runtime-legacy.actual
git diff --check
```

Expected: every Task 01 concurrency characterization is GREEN. Resource/host
cases explicitly assigned to Tasks 05–07 may remain outside this command, not
ignored inside it.

- [ ] **Step 3: Assign independent review**

Reviewer finding IDs use `UR-T04-R###`. Review builds an ownership graph for
every public form; injects value/error/cancel settlements at each boundary;
checks loser/sibling survival for observations; checks zero children after owned
forms; and searches for nested drive loops, dummy-yield `nil`, and string-parsed
cancellation.

- [ ] **Step 4: Fix findings test-first and rerun all gates**

Add each discovered edge case to `async_contract_test.rs` or
`async_owned_test.rs` before the production fix.

- [ ] **Step 5: Commit the accepted layer**

```bash
git add crates/sema-core crates/sema-vm crates/sema-stdlib crates/sema-eval \
  crates/sema/tests docs/internals/async-runtime-inventory.md \
  docs/plans/evidence/unified-cooperative-runtime \
  docs/plans/reviews/unified-cooperative-runtime
git commit -m "feat(runtime): add explicit async ownership semantics"
```

## Completion criteria

- All public observational and owned APIs implement the table exactly.
- Promise states partition returned, failed, cancelled, and pending.
- Supplied promises survive observational waiter failure/cancel/timeout.
- Owned scopes have zero live children when their API settles.
- `race` uses settlement order even for pre-settled reverse-order inputs.
- `async/run` is an origin-root barrier, not a nested scheduler.
- Duration and capacity edge cases return conditions without panic/allocation.
- Channels use generation-safe runtime waits and trace buffered values.
- Every cancellation-dependent old timeout use is intentionally migrated.
- Independent review and durable evidence are clean.
