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
  fair bounded driving, generations, cleanup registry, captured-cell coherence.
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

- `crates/sema-vm/src/runtime/promise.rs` — runtime promise registry and
  observations.
- `crates/sema-vm/src/runtime/scope.rs` — owned child scopes and cleanup state.
- `crates/sema-vm/src/runtime/channel.rs` — bounded channel state and waits.
- `crates/sema-stdlib/src/async_owned.rs` — thunk-taking structured operations.
- `crates/sema/tests/async_contract_test.rs` — language contract matrix.
- `crates/sema/tests/async_owned_test.rs` — ownership/reaping matrix.
- `crates/sema/tests/async_condition_test.rs` — catch/rethrow/trace behavior.
- `docs/plans/evidence/unified-cooperative-runtime/task-04.md` — exact API and
  cleanup evidence.
- `docs/plans/reviews/unified-cooperative-runtime/task-04.md` — independent
  review report.

**Modify**

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
    Settled(TaskSettlement),
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
    pub primary: Option<TaskSettlement>,
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

## Task 3: Implement observational `all`, `race`, and `timeout`

**Files:** `async_ops.rs`, `runtime/promise.rs`, `runtime/wait.rs`,
`vm_async_test.rs`, `async_contract_test.rs`

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

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test async_contract_test -- channel
cargo test -p sema-core cycle
```

Expected: channel matrix passes without polling or wall-clock sleeps.

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

Expected: cancellation maps contain `:type`, `:reason`, `:task-id`, and
`:root-id`; timeout maps contain `:type` and `:duration-ms`; rethrow preserves
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
