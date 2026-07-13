# Unified Cooperative Runtime Rewrite

> **Status:** Approved architecture and expanded execution plan.
> **Implementation status:** No production rewrite layer is accepted yet. The
> characterization commit at `52293e61` is provisional and must be corrected as
> described in [Task 01 status](#task-01-status).

This specification defines the runtime that every implementation task must
produce. The ordered task files execute it as horizontal layers; they may not
change these semantics without first updating and re-approving this document.

## Outcome

Replace Sema's split, re-entrant async machinery with one interpreter-owned
cooperative runtime. Every unit of Sema execution is a runtime task. Every
operation that cannot finish immediately suspends that task through the same
runtime, regardless of whether it originated in a native function, callback,
timer, channel, file operation, process, server, LLM tool, MCP call, host eval,
notebook cell, debugger request, or WASM callback.

The rewrite is a hard cut. The completed branch contains no compatibility
scheduler, alternate evaluator, host replay path, or feature flag that retains
the old machinery.

## Priorities and release policy

The work is optimized for autonomous implementation and review agents, not for
human-sized diffs or incremental release. Task size is not a constraint.

Priorities are ordered:

1. Correctness, cancellation safety, and resource cleanup.
2. A coherent architecture with explicit ownership and maintainable boundaries.
3. Deterministic tests, diagnostics, and reviewability.
4. Performance.

Every horizontal layer must compile and pass its local structural gates so later
agents receive useful feedback. Cross-layer conformance tests may remain RED
until their owning layer lands. The branch is nevertheless unreleasable until
all layers, legacy-deletion gates, adversarial tests, independent review rounds,
and final profiling work are complete.

Do not profile or tune between implementation layers. Layer-level performance
checks exist only to catch hangs, leaks, or an obvious accidental complexity
explosion. Comprehensive profiling and benchmark comparison happen after the
entire functional migration and correctness campaign are green.

## Normative language

`MUST`, `MUST NOT`, `SHOULD`, and `MAY` are requirements in the RFC sense.
Implementation details not fixed here may change if they preserve every
observable contract and verification gate.

## Terms

| Term | Meaning |
| --- | --- |
| Runtime | The single scheduler, task store, wait system, and external-event bridge owned by an interpreter. |
| Root | One host-submitted evaluation with a stable `RootId`, result handle, cancellation scope, output sink, and initial task context. |
| Task | One resumable Sema computation with a stable `TaskId`. |
| Settlement | A task's single terminal transition to a returned value, failure, or cancellation. |
| Observation | Waiting for a task through a promise without acquiring ownership of that task. |
| Lifetime ownership | Responsibility for retaining, cancelling when required, and reaping a task or operation. |
| Cancellation ancestry | The independent relationship that determines where a cancellation request propagates. |
| Reaping | Removing a settled task and all of its live waits/continuations from its owner after its observable settlement has been retained. |
| Quarantine | Runtime-owned tracking for a bounded external operation that cannot be interrupted but can no longer resume Sema. |
| Cooperative boundary | A point where the runtime may suspend, preempt, cancel, or switch the active task. |

## Non-negotiable contracts

- `Interpreter` owns exactly one runtime. The bytecode VM remains the sole
  evaluator.
- A host eval is a root task. Nested evaluation never starts another scheduler
  or removes the running task from the task table.
- Multiple roots MAY be active concurrently in one interpreter.
- Worker threads and host callbacks MUST NOT execute Sema, mutate the VM, or
  hold `Value`, `Env`, `EvalContext`, task frames, or other `Rc`-backed runtime
  state.
- Every task, root, wait, and settlement has stable checked identity. IDs and
  sequence counters never wrap or reuse a live identity.
- Cancellation, lifetime ownership, and observation are separate relationships.
- Supplied promises are observed, never implicitly adopted.
- Captured lexical mutation uses shared traceable cells. Spawning MUST NOT clone
  captured cell state.
- Canonical task context lives in task records. Ambient thread-local state is at
  most a temporary adapter installed from the active task for an external
  library call.
- Browser execution uses genuine suspension and host wakeups. It MUST NOT use
  evaluation replay, replay markers, synchronous XHR, `Atomics.wait`, or a
  host-side scheduler clone.
- No merge, tag, package, or release is allowed until the definition of done is
  satisfied.

## Runtime architecture

### Runtime-owned components

The runtime owns these independently testable components:

- `RootStore`: root state, result handles, cancellation scopes, output sinks,
  and root-local context.
- `TaskStore`: task state, VM frames, native continuations, context, ownership,
  cancellation ancestry, and settlement records.
- `ReadyScheduler`: active-root rotation plus per-root FIFO task queues and a
  per-task queued bit.
- `TimerQueue`: monotonic deadlines ordered by deadline and insertion sequence.
- `WaitRegistry`: the sole authority connecting tasks to promises, timers,
  channels, resources, and external operations.
- `CompletionInbox`: thread-safe completion messages containing only sendable
  host data.
- `CleanupRegistry`: quarantined bounded operations and one-shot cleanup hooks
  that outlive a cancelled task.
- `RuntimeClock`: monotonic production time and deterministic virtual time for
  tests.
- `DebugCoordinator`: runtime-wide pause state and stable task inspection.
- `ShutdownState`: admission control, cancellation, drain progress, and the
  final cleanup report.

The components expose narrow interfaces. No subsystem-specific scheduler loop,
task registry, timer loop, or completion poller may remain.

### Roots

`submit_root` creates a monotonic `RootId`, a root task, and a `RootHandle`.
Roots share the runtime and global environment but have separate:

- result handles;
- cancellation scopes;
- output/event sinks;
- initial dynamic, sandbox, tracing, and usage context;
- host metadata and diagnostics.

A root's result may settle while detached tasks originating from that root are
still pending. Normal root settlement does not cancel those tasks. The root
record remains alive while a result handle, descendant task, output sink, or
debug/tracing record still requires it.

An explicit root cancellation propagates to every pending task in that root's
cancellation ancestry, including detached tasks whose originating root already
settled. It does not affect tasks originating from another root. An explicit
`async/cancel` through a shared promise is a separate cancellation capability
and may cross roots because the holder deliberately possesses that handle.

### Shared global environment

Concurrent roots share one global Sema environment. Individual environment and
shared-cell operations are atomic with respect to cooperative scheduling; a
task never switches halfway through one such operation. If roots write the same
binding, the last scheduled write wins. Compound read-modify-write behavior
requires an explicit atomic/shared-cell primitive when lost updates matter.

Reproducibility is defined for the same source, initial state, scheduler
configuration, and external-event ordering. Network and host callback arrival
order is an input, not hidden nondeterminism. Applications that require
environment isolation use separate interpreters rather than implicit root
snapshots or transactions.

### Task lifecycle and settlement

A task has one lifecycle state:

```text
Ready -> Running -> Ready
             |  -> Waiting(wait_id, generation) -> Ready
             |  -> Settled(Returned(Value))
             |  -> Settled(Failed(SemaError))
             `  -> Settled(Cancelled(CancelReason))
```

Cancellation request state is recorded separately from lifecycle state so a
waiting operation can be interrupted and cleaned up before terminal settlement.
Every transition is validated centrally.

The following are invariant violations:

- settling a task twice;
- executing or enqueueing a settled task;
- enqueueing a task more than once;
- registering one task in multiple active waits;
- delivering a completion to the wrong task or wait generation;
- retaining an untraced `Value` in a task, continuation, promise, channel, or
  runtime registry;
- losing a task while it is running;
- wrapping or reusing a live task, root, wait, generation, or settlement ID.

Every settlement receives a runtime-wide monotonic `SettlementSeq`. Synthetic
promises created by `async/resolved` and `async/rejected` receive a sequence when
created. The sequence determines first-settlement behavior when an observer sees
multiple already-settled promises. Checked counter exhaustion is a controlled
runtime failure; counters never wrap.

### Independent task relationships

Each task records separate relationships:

| Relationship | Purpose | Example |
| --- | --- | --- |
| Origin root | Output, tracing, fair scheduling, and root cancellation | A detached `async/spawn` task retains the root that created it. |
| Cancellation parent | Downward cancellation propagation | A scoped map child is cancelled with its owning map operation. |
| Lifetime owner | Retention and reaping | The interpreter owns detached tasks; a combinator scope owns its direct children. |
| Observer registrations | Result delivery only | `async/all` observes supplied promises without owning them. |

No operation may infer ownership merely from possession of a promise. Sema does
not implicitly adopt external tasks.

### Fair scheduling

Scheduling is FIFO within a root and round-robin across active roots. A root that
creates thousands of ready tasks cannot bury an interactive root behind its
whole queue. Detached tasks remain in their origin root's scheduling bucket.

One task dequeue runs for at most a configurable VM reduction budget, initially
10,000 instructions, or until the task returns, fails, suspends, yields, or
observes cancellation. Tests inject small budgets to force interleavings;
correctness MUST NOT depend on the production budget.

The runtime processes bounded batches from the inbox and timer queue between
task quanta. Completion storms, due-timer storms, and ready-task storms must each
leave progress opportunities for the other sources. Timer ties use insertion
sequence. External completion order is the order accepted by the inbox.

There is no global tick ceiling. Watchdogs belong to tests and hosts, not to
normal scheduler correctness.

### Drive turns

A host drive turn has a separate reduction/task budget and wall-clock budget.
Within that bound it repeatedly:

1. Accepts a bounded batch of external completions.
2. Expires a bounded batch of due timers.
3. Rotates to the next ready root and runs one ready task quantum.
4. Applies the task transition, registers waits, records settlements, and wakes
   exact dependants.
5. Performs bounded cleanup progress.
6. Repeats while the host budget remains.

`drive` returns host-facing state rather than blocking inside the evaluator:

- ready work remains and another turn should be scheduled;
- no task is ready, with the next timer deadline and inbox wake requirement;
- the runtime is quiescent;
- shutdown completed or failed its invariant checks.

Hosts observe requested root results through `RootHandle`; drive turns are not
owned by one root. A native blocking eval wrapper may wait for one root while
advancing every root and detached task fairly.

### Native call and continuation protocol

Runtime-aware native calls return:

```text
Result<NativeOutcome, SemaError>

NativeOutcome = Return(Value)
              | Call {
                  callable,
                  args,
                  continuation
                }
              | Suspend {
                  suspension,
                  continuation
                }
```

`Call` pushes ordinary Sema work onto the active task. Higher-order functions,
LLM tools, server handlers, stream callbacks, and other callback users MUST use
this protocol instead of synchronously invoking a global evaluator callback.

`Suspend` registers one wait and stores a `NativeContinuation` on the task's
traceable frame stack. A continuation is one-shot, explicitly stateful, and
independently testable. Every field containing a Sema value participates in the
CORE-2 cycle collector. Native closures continue to obey invariant I2: traceable
state belongs in registered payloads, while host infrastructure uses weak
references.

When a wait completes, the runtime places decoded VM-thread data into the exact
continuation and requeues the task. Native code never reruns from the beginning
to simulate resumption.

### Waits and completion delivery

Every suspension creates a stable `WaitId` and generation owned by the
`WaitRegistry`. Completion succeeds only if all of these still match:

- runtime instance;
- task identity;
- wait identity and generation;
- task lifecycle state;
- expected completion kind.

Cancellation unregisters the active wait before the task can run again. Late,
duplicate, stale, or reordered completions are ignored and recorded in debug
metrics; they never access task frames or Sema values.

Required wait families include:

- task/promise settlement;
- timers and deadlines;
- channel send/receive/close;
- worker-pool jobs;
- files, streams, databases, KV, archives, PDFs, git, serial, and secrets;
- processes and PTYs;
- sockets, HTTP, WebSocket, and server request/disconnect events;
- LLM requests, streams, retry timers, tools, and usage delivery;
- MCP connection queues, requests, and cassette paths;
- native host callbacks and WASM futures.

### Worker-thread boundary

External workers receive owned, `Send`-safe input and return owned, `Send`-safe
completion payloads. They MUST NOT receive or return `Value`, `Env`,
`EvalContext`, VM/native frames, task-local guards, `Rc`, or pointers into the
interpreter.

All conversion to and from Sema values, continuation execution, promise
settlement, tracing, and GC interaction occurs on the interpreter thread.
Compile-time types should make the boundary difficult to violate; source scans
and focused compile-fail or trait tests guard it.

### Cancellation

Cancellation is a sticky request with an explicit `CancelReason`. Requesting it
is idempotent. A pending task observes cancellation at its next cooperative
boundary; a waiting task first deregisters or cancels its wait. CPU-bound tasks
check at quantum boundaries.

Cancellation is internally distinct from failure. At the language boundary it
is a structured, catchable `:cancelled` condition. Catching the condition may
inspect it and perform bounded cleanup, but it does not silently clear the
sticky cancellation request or convert interpreter shutdown into success. No
implicit uncancel operation is part of this rewrite.

A handler that catches cancellation runs in cancellation-cleanup mode:
synchronous bounded cleanup may run, attempting to suspend observes the same
cancellation immediately, and returning from the handler still settles the task
as cancelled. This makes cancellation inspectable without letting a broad
catch accidentally defeat root cancellation or shutdown.

`async/cancel` returns `#t` only when it records the first cancellation request
for a pending spawned task. It returns `#f` for synthetic promises, already
requested tasks, and terminal tasks. The promise becomes terminally cancelled
after task cleanup reaches its settlement point; until then it remains pending
with cancellation requested.

Root cancellation, owner cancellation, explicit promise cancellation, timeout,
host stop, resource disconnect, and interpreter shutdown use distinct reasons.

### Resource cancellation classes

Every in-flight operation declares and tests one implementation class:

| Class | Meaning | Required behavior | Typical examples |
| --- | --- | --- | --- |
| Interruptible | The underlying local operation can be stopped or deregistered. | Invoke an idempotent cancel hook, release local resources, reject late delivery, then settle cancellation. | Timer/channel wait, closable socket wait, child process with a kill handle. |
| Quarantined bounded | The operation cannot be interrupted but has a proven hard deadline or finite work bound. | Detach it from Sema, transfer it to `CleanupRegistry`, discard its result, and reap it within its declared bound. | Small finite worker computation or a library call with an enforced deadline. |
| Unbounded non-interruptible | The operation may never return and cannot be stopped. | Prohibited. Replace the API, add an enforceable deadline, close/kill its resource, or put it behind a killable process boundary. | Unclosable pipe read, request without a timeout, subprocess wait without a kill handle. |

“Normally quick” is not a bound. Every quarantined operation records its bound
and appears in shutdown diagnostics.

Task reaping and external-operation cleanup are distinct. A cancelled task may
be reaped after its quarantined job is safely transferred to runtime ownership;
the runtime remains responsible for the job until it finishes. This transfer is
not an untracked leak. Owned scopes wait for child settlement and successful
transfer, not for the quarantined external job to finish.

Cancelling a wait does not automatically close a shared resource. The operation
contract decides whether to deregister one waiter, interrupt one operation, or
close an exclusively owned resource. Resource handles returned as Sema values
retain their normal explicit/GC lifetime unless a scope explicitly owns them.
Close, cancel, disconnect, and drop paths are idempotent.

Remote cancellation is necessarily limited by the remote protocol. Sema MUST
stop local delivery and dispatch the strongest protocol-supported cancellation,
but documentation must not claim that a remote provider stopped billing unless
that provider guarantees it.

### Shutdown

Explicit interpreter shutdown is the verified lifecycle operation:

1. Reject new roots and spawns.
2. Request cancellation for every pending root and detached task.
3. Deregister waits and invoke one-shot cancellation hooks.
4. Drive cancellation, task settlement, and quarantine cleanup.
5. Stop worker infrastructure within its declared operation bounds.
6. Verify that task, wait, timer, completion, resource, and cleanup registries
   contain no live operation.
7. Return a structured shutdown report.

Shutdown MUST have a bounded host contract and cannot hang forever. Exceeding a
declared bound is an invariant failure with diagnostics, not silent success.
Rust `Drop` remains panic-safe and best-effort because it cannot report failure;
hosts and tests use explicit shutdown whenever full cleanup is required.

### Memory and cycle collection

The runtime does not replace Sema's `Rc` plus CORE-2 cycle-collection model.
Live task frames, continuations, promises, channels, shared cells, root context,
and other runtime-held Sema values are collector roots and expose their traced
edges. Settled values remain reachable through promises/result handles but dead
task execution state does not.

Worker payloads are outside the Sema object graph by construction. Stress tests
must prove collection across task/promise/channel/continuation cycles and heavy
spawn, cancel, settle, and handle-drop churn.

## Language-facing concurrency contract

### Promise states

Promises have four observable states: pending, returned, failed, and cancelled.
The existing predicates partition these states; cancellation never aliases a
rejection whose text happens to contain “cancelled.” A promise may have multiple
observers. Dropping every observer does not cancel an interpreter-owned detached
task.

Awaiting a returned promise yields its value. Awaiting a failed promise raises
its preserved failure. Awaiting a cancelled promise raises a structured
`:cancelled` condition.

### Observational operations

Promise-taking operations do not own their inputs:

| Operation | Contract |
| --- | --- |
| `(async/await promise)` | Wait for one promise. Cancelling the waiter removes only its observation. |
| `(async/all promises)` | Return values in input order. Empty input returns an empty list. The lowest-sequence failure or cancellation observed before completion is raised immediately; other supplied tasks continue. |
| `(async/race promises)` | Require at least one promise. The lowest `SettlementSeq` wins, whether it returned, failed, or was cancelled. Losers continue. Duplicate promise entries are valid observations of the same settlement. |
| `(async/timeout ms promise)` | Bound how long this caller observes the promise. A pending promise at deadline raises `:timeout`; the supplied task continues. An already-settled promise wins even when `ms` is zero. |

Cancelling a task blocked in any observational operation removes its observation
registrations and leaves every supplied producer unchanged.

### Detached spawning

`(async/spawn thunk)` creates an interpreter-owned detached task and returns its
promise. The task inherits the current root as its cancellation origin and fair
scheduling bucket. It may survive normal root settlement and be awaited by a
later root. Explicit origin-root cancellation, explicit promise cancellation,
or interpreter shutdown still cancels it.

Unhandled detached-task failures remain available through their promises. A
debug/host diagnostic is emitted whenever a failed settlement has no remaining
observation handle; if the final handle was dropped while pending, the diagnostic
is emitted when the task later fails.

### Owned operations

Thunk-taking operations own the work they create:

| Operation | Contract |
| --- | --- |
| `(async/spawn-all thunks)` | Spawn all direct children, return values in input order, and return an empty list for empty input. First failure/cancellation cancels and reaps unfinished children before propagation. |
| `(async/map f items)` | One owned child per item, unbounded fan-out, input-order results, and the same fail-fast cleanup as `spawn-all`. |
| `(async/pool-map f items n)` | At most `n` calls to `f` active, input-order results, no more worker tasks than useful concurrency, and the same fail-fast cleanup. `n <= 0` is an argument error. |
| `(async/race-owned thunks)` | Require at least one thunk. Record the first settlement as winner, cancel and reap every losing child, then return or raise the preserved winning outcome. |
| `(async/with-timeout ms thunk)` | Create one owned child. If the deadline wins, cancel and reap/transfer its work before raising `:timeout`. If the child settles first, preserve its outcome. |

For an owned failure race, the lowest settlement sequence is the primary
outcome. Cleanup errors are attached as suppressed diagnostics; they never mask
the primary outcome. Runtime cleanup invariant failures still fail verification
and shutdown.

Higher-level `parallel`, `pipeline`, settled variants, workflow fan-out, and
agent orchestration must be rebuilt on these ownership rules. Fail-fast forms
cancel owned siblings. Settled/collect-all forms retain each item outcome and do
not cancel siblings merely because one item failed; parent cancellation still
cancels them all.

### `async/run`

`(async/run)` is a barrier for detached tasks originating from the current
root. It suspends the caller until every other pending task with that origin
root, including transitively spawned descendants, settles. It does not wait for
unrelated roots and does not start a nested scheduler. It returns `nil` and
preserves individual results/errors on their promises; unobserved failures use
the detached-task diagnostic rule.

### Duration and capacity validation

Duration parsing is shared by sleep and timeout operations. Negative values are
rejected before rounding. NaN, infinity, overflow, and values beyond the
documented maximum return a Sema condition without panic or accidental long
sleep. Sub-millisecond rounding is documented and identical on native and WASM.

Channel capacity is validated before conversion and allocation. Zero, negative,
unrepresentable, and allocation-impossible capacities return a Sema condition;
they never panic or attempt an enormous allocation.

### Structured conditions

Cancellation and timeout become first-class `SemaError`/condition categories,
not `Eval` strings that callers must parse. Caught values include stable fields:

```sema
{:type :cancelled
 :message "task cancelled"
 :reason :root-cancelled
 :task-id 42
 :root-id 7}

{:type :timeout
 :message "operation exceeded 30000 ms"
 :duration-ms 30000}
```

Normal errors remain concise. Debug mode may add wait kind, owner, cancellation
source, and logical async ancestry without changing condition identity.

Async stack traces preserve execution frames plus logical `spawned by`,
`awaited by`, and `cancelled by` links. Re-raising a condition preserves its map
and trace semantics.

## Task context and inheritance

Canonical context is explicit in each task record and installed on every resume
through a panic-safe guard. Sibling tasks never inherit mutations accidentally.

| Context | Child behavior |
| --- | --- |
| Captured lexical bindings | Share the same traceable cells. |
| User and hidden dynamic-context frames | Copy the current frame structure as a task-private snapshot. |
| Context stacks and R7RS parameters | Snapshot values; later push/pop/set is task-private unless the value is itself an explicit shared cell/resource. |
| Sandbox/capabilities | Inherit the same or a narrower capability set; never widen. |
| Current file and module-load stack | Snapshot as task-private state. |
| Module cache and global environment | Share interpreter-owned state through atomic cooperative operations. |
| Output sink | Share the origin root sink; tag events with root/task/sequence identity. |
| Tracing | Create explicit child-span linkage; never rely on whichever thread-local span is active. |
| LLM cache/provider/tags configuration | Snapshot immutable configuration. |
| Usage and budget scope | Share the active accounting object so concurrent children charge one aggregate. |
| “Last usage,” retry cursor, streaming cursor | Task-private. |
| Workflow run context and MCP handles | Share the workflow/resource-owned handle while keeping active guards and request state task-private. |
| Debugger state | Runtime-owned task identity and frames; not copied ambient state. |

External libraries that require thread-local guards may receive a guard installed
from the active task for the duration of one runtime step. The task record remains
the source of truth. Suspension inside user, hidden, file/module, sandbox,
workflow, tracing, usage, budget, LLM, and MCP scopes is a required leakage test.

## Host contract

### Common API

All hosts use the same conceptual operations:

```text
submit_root(source_or_value, root_options) -> RootHandle
drive(DriveBudget)                         -> DriveState
cancel_root(RootId, CancelReason)          -> bool
shutdown(ShutdownOptions)                  -> ShutdownReport
```

`RootHandle` exposes identity, nonblocking result inspection, completion wakeup,
and explicit cancellation. `RootOptions` supplies the output sink, sandbox,
initial task context, tracing parent, and host metadata.

The interpreter and VM remain owned by one host thread. Sendable runtime handles
may only enqueue cancellation, wakeup, or external-completion data.

### Native blocking wrappers

Synchronous native `eval` submits one root, drives the shared runtime, and parks
on the inbox or next timer only when no ready work exists. It returns when its
requested root settles, not when every detached task settles. While waiting, it
advances all roots fairly.

CLI, file execution, build, REPL, embedding, DAP, LSP, notebook, MCP server, and
workflow entry points must use this contract rather than private scheduler loops.

### Browser and WASM

WASM `eval()` returns a JavaScript `Promise`; `evalAsync()` MAY remain as an
alias. Multiple eval promises may be pending simultaneously. Host callbacks
enqueue completion data and schedule bounded future drive turns.

Sustained work must yield through a macrotask/event-loop mechanism that permits
rendering, input, timers, fetch, and cancellation to progress. A self-perpetuating
Promise-microtask chain is not sufficient. No evaluation replay or synchronous
wait fallback is permitted.

Playground Stop cancels the exact root. Output events are root-tagged so
concurrent evaluations cannot mix ownership even if the UI chooses to render
them together.

### Debugger behavior

A breakpoint establishes a runtime-wide debug barrier after the active task
reaches a valid transition point. User tasks stop while DAP inspection reads
frames and scopes; external completions may queue but are not delivered into
tasks. Step commands advance only the selected task according to DAP semantics.
Resume releases the barrier and normal root fairness continues.

This stop-the-world inspection rule keeps shared globals and frame graphs stable
while debugging multiple roots.

## Complete migration scope

`docs/internals/async-runtime-inventory.md` is the executable migration ledger.
It must name every production site, cancellation class, context policy, test,
legacy-removal symbol, and host. A checked item means the path uses the unified
runtime or has a reviewed proof that it is strictly synchronous and cannot call
or suspend Sema.

At minimum the inventory covers:

### Core, evaluator, and VM

- `sema-core`: `async_signal.rs`, `io_backend.rs`, `context.rs`, `value.rs`,
  `cycle.rs`, `mcp_cassette.rs`, and exports.
- `sema-vm`: scheduler, VM dispatch/call frames, debugger, stack traces, and all
  yield/preemption paths.
- `sema-eval`: interpreter/root APIs, callback delegates, prelude concurrency
  macros, module/file context, and debug session state.
- `sema-io`: worker-pool submission, job identity, completion delivery, and
  shutdown.

### Standard library and resources

- `async_ops.rs`, all channel/promise operations, context, list/map/meta/string,
  system, typed arrays, workflow, workflow MCP, and OTel callback users.
- Archive, diff, event, git, HTTP, file I/O, KV, PDF, process, PTY, secret,
  serial, server, SQLite, stream, terminal, and WebSocket modules.
- Every resource close/drop/cancel/disconnect path and every direct or indirect
  `in_async_context`, yield-signal, polling, blocking-receive, sleep, or callback
  branch.

### LLM, agents, workflows, MCP, and tracing

- Provider completion, chat, embedding, streaming, retry, timeout, cache,
  fallback, budget, usage, and tracing paths.
- Multi-round agent and tool callbacks, streaming callbacks, and cleanup.
- Workflow fan-out, journals, budgets, MCP handle resolution, and resume.
- MCP connect/auth/list/call, per-connection serialization, cassettes, reconnect,
  server tools, and cancellation.
- `sema-otel` span/context bridges and shutdown/export paths.

### Hosts and shipped web boundary

- CLI expression/file/build, REPL, embedded Rust API, DAP, LSP, notebook, MCP
  server, workflow entry points, and tests that host interpreters.
- `sema-wasm`, playground worker/client, generated and vendored WASM assets, and
  packaged browser-boundary tests.

### Tests, docs, examples, and generated surfaces

- Every async/concurrency/integration test under `crates/sema/tests` and affected
  crate-local tests.
- Async, server, LLM, MCP, workflow, notebook, debugger, and playground examples.
- Builtin documentation, generated docs index, evaluator/runtime architecture
  docs, limitations/deferred records, and shipped web assets.

Inventory discovery is repeated after migration with source scans; the initial
list is not assumed complete merely because it was written before implementation.

## Horizontal implementation layers

The ordered task plans implement broad layers. Intermediate layers may include
temporary adapters solely to preserve compilation and feedback. Every adapter
must have an owner and deletion gate; none is releaseable.

1. **Contracts and inventory.** Correct characterization tests, executable
   invariants, complete the migration ledger, and install static guard scaffolds.
2. **Core runtime data model.** Roots, tasks, settlements, ownership,
   cancellation ancestry, waits, completion payloads, task context, tracing
   edges, and checked identities.
3. **Scheduler and interpreter.** Root-fair queues, quanta, timers, inbox,
   continuations, VM/evaluator integration, drive API, virtual time, GC, shutdown,
   and debugger barrier.
4. **Concurrency language layer.** Promises, spawn/await/run, observational and
   owned combinators, cancellation, timeouts, timers, channels, shared cells,
   and higher-level fan-out macros.
5. **I/O and resources.** Worker pool, files, streams, databases, processes,
   PTYs, sockets, HTTP/WebSocket, servers, and every resource cancellation class.
6. **Context-sensitive integrations.** Task-local inheritance, LLM, agent tools,
   streaming, usage/budgets, workflows, MCP queues/cassettes, and OTel.
7. **Hosts.** CLI, REPL, embedding, DAP, LSP, notebook, MCP server, workflows,
   WASM, playground, concurrent roots, output routing, and packaged boundaries.
8. **Legacy deletion and documentation.** Remove all adapters and old machinery,
   enforce source guards, update docs/examples, and regenerate derived assets.
9. **Integrated adversarial verification.** Deterministic and real-resource
   stress, fault injection, leak/GC checks, host/WASM testing, and full CI gates.
10. **Independent review campaign.** Multiple branch-wide specialist reviews,
    finding reduction, remediation, and re-review until no correctness issue
    remains.
11. **Final profiling and benchmarking.** Compare with the pinned pre-rewrite
    runtime, investigate regressions, tune where justified, rerun correctness and
    review gates after changes, then record final confirmation benchmarks.

There is no per-layer profiling task.

The executable task files are:

1. [Task 01 — contracts, characterization, and inventory](2026-07-13-unified-cooperative-runtime-task-01.md)
2. [Task 02 — core runtime data model](2026-07-13-unified-cooperative-runtime-task-02.md)
3. [Task 03 — scheduler, interpreter, and VM continuations](2026-07-13-unified-cooperative-runtime-task-03.md)
4. [Task 04 — language concurrency and structured ownership](2026-07-13-unified-cooperative-runtime-task-04.md)
5. [Task 05 — interruptible I/O and bounded resources](2026-07-13-unified-cooperative-runtime-task-05.md)
6. [Task 06 — task context and orchestration integrations](2026-07-13-unified-cooperative-runtime-task-06.md)
7. [Task 07 — native, service, notebook, debugger, and WASM hosts](2026-07-13-unified-cooperative-runtime-task-07.md)
8. [Task 08 — legacy deletion, docs, examples, and shipped assets](2026-07-13-unified-cooperative-runtime-task-08.md)
9. [Task 09 — adversarial, model, fuzz, stress, and leak verification](2026-07-13-unified-cooperative-runtime-task-09.md)
10. [Task 10 — six-round independent review campaign](2026-07-13-unified-cooperative-runtime-task-10.md)
11. [Task 11 — final profiling, benchmarking, and release readiness](2026-07-13-unified-cooperative-runtime-task-11.md)

## Verification strategy

### Per-layer gates

Each task defines exact commands, but every implementation layer must include:

- focused unit and conformance tests for its contracts;
- deterministic virtual-time/interleaving tests where applicable;
- source guards for forbidden shortcuts and temporary-adapter accounting;
- formatting, linting, and build checks for affected targets;
- an implementer self-review and an independent maintainability/correctness
  review;
- durable evidence containing commands, results, known RED cross-layer tests,
  and the next layer's handoff state.

Local gates do not benchmark throughput. They do reject hangs, leaks, unbounded
queues, accidental blocking, and obviously superlinear behavior where the
contract requires bounded work.

### Bug remediation loop

Every discovered defect follows this sequence:

1. Assign a stable finding ID and record evidence.
2. Reduce it to the smallest deterministic reproduction.
3. Add a regression test that fails for the intended reason.
4. Fix the implementation without weakening the oracle.
5. Run affected suites and the complete integration suite.
6. Have a different review agent verify the reproduction, fix, and test.

Reports live under
`docs/plans/reviews/unified-cooperative-runtime/`; execution evidence lives under
`docs/plans/evidence/unified-cooperative-runtime/`. `/tmp` reports are scratch
only and never satisfy a task gate.

### Required adversarial matrix

The integrated campaign covers at least:

- cancellation at every call, quantum, wait registration, wake, settlement,
  ownership, quarantine-transfer, and cleanup boundary;
- duplicate, stale, late, reordered, wrong-kind, and wrong-generation
  completions;
- multiple roots starting, mutating globals, producing output, completing,
  failing, and cancelling in every ordering;
- empty inputs, duplicate promises, already-settled promises, simultaneous
  settlements, ID/generation exhaustion seams, and extreme task churn;
- more than one million finite yields without a scheduler tick ceiling;
- perpetual ready work alongside timers and external completion, always guarded
  by an out-of-process or host watchdog that cannot become an in-process hang;
- channel FIFO, close, backpressure, cancelled waiter, capacity, and producer/
  consumer races;
- slow consumers, disconnects, partial reads/writes, broken pipes, killed
  processes, PTY teardown, server cancellation, and shutdown;
- provider errors, streaming interruption, retry/timeout races, budget charging,
  MCP queue/reconnect/cassette behavior, and workflow cancellation;
- nested callback/native chains that spawn, await, sleep, perform I/O, fail, and
  preserve the parent task;
- task-local leakage, sandbox widening, output misrouting, incorrect usage
  attribution, trace corruption, and debug pause/resume;
- promise/task/channel/continuation cycles and interpreter-drop stress;
- native and WASM fairness with browser heartbeat/render/input checks;
- seeded randomized scheduling, property tests, targeted model checking, and
  repeated debug/release stress runs.

Real-network tests use controlled local servers or deterministic fake providers
unless a separately recorded live smoke is required. Every real-time test has a
hard external watchdog and conservative timing assertions.

### Static removal and boundary guards

Production source scans fail on reintroduction of legacy or blocking paths,
including the final removed forms of:

- `IoHandle`, `IoPoll`, `YieldReason`, scheduler targets/results, yield-signal
  setters, resume-value TLS, and re-entrant run helpers;
- evaluator/cancel/spawn scheduler callbacks and temporary running-task removal;
- subsystem-specific polling or `block_on` in a runtime task path;
- direct `thread::sleep`, blocking channel receive, or nested scheduler drive in
  evaluator/runtime code;
- worker payloads containing Sema runtime values;
- browser replay markers/limits, synchronous `XMLHttpRequest`, `Atomics.wait`,
  and host-side scheduler loops.

Any allowlist entry names an exact file, reason, owner, and deletion or permanent
synchronous-proof decision. Broad substring exclusions are forbidden.

### Independent review rounds

After all functional layers and integrated tests are green, independent agents
perform separate reviews for:

1. Architecture, state-machine, type-boundary, and inventory consistency.
2. Cancellation, ownership, resource lifecycle, shutdown, and leak safety.
3. Fairness, determinism, timers, channels, and hostile interleavings.
4. Task-local context, sandbox/security, LLM usage, workflows, MCP, and tracing.
5. Native hosts, debugger/tooling, WASM, browser event-loop behavior, and shipped
   asset boundaries.
6. Complete final diff, documentation, maintainability, and deletion gates.

No correctness, safety, leak, or nondeterminism finding may be waived. Other
findings require an explicit disposition. Any remediation receives regression
coverage and re-review; the finding is not closed by the implementing agent.

## Final profiling and benchmarking

Profiling starts only after layers 1–10 are functionally complete and green. The
comparison baseline is the last pre-rewrite production-code commit, recorded
before the first production layer begins. At the time of this specification the
candidate is `3f111e83`; the task plan must update the recorded SHA if production
code changes before the rewrite starts.

The final campaign compares identical builds, fixtures, hardware metadata, and
measurement settings for the baseline and completed runtime. It covers:

- root submission and single-root eval overhead;
- spawn, yield, await, settlement, cancellation, and task churn;
- one-root and many-root fairness latency;
- timer throughput, drift, and wake latency;
- channel throughput, contention, backpressure, and memory;
- file/stream/database/process/PTY/socket/HTTP/WebSocket/server workloads;
- LLM fake-provider concurrency, agent tool loops, streaming, MCP queues, and
  workflow fan-out;
- Sema Coder and representative notebook/server/workflow end-to-end scenarios;
- allocation, retained memory, cycle collection, shutdown, and leak plateaus;
- WASM throughput, bundle/runtime memory, event-loop heartbeat, and input/render
  latency.

Store raw results, statistical summaries, profiles/flamegraphs, benchmark source,
baseline SHA, toolchain, and environment metadata as durable evidence.

Regressions must be understood, not automatically hidden or optimized away.
Correctness and maintainability outrank performance. Minor justified regressions
may be accepted with written evidence; broad, severe, asymptotic, or unexplained
regressions require profiling and a tuning attempt. Do not restore tangled or
unsafe machinery merely to match an old number.

Performance changes restart affected correctness suites and specialist review.
The final sequence is:

1. Profile and benchmark the correctness-complete implementation.
2. Investigate and tune justified hotspots.
3. Rerun the complete correctness, stress, leak, and review gates.
4. Rerun clean confirmation benchmarks on the reverified code.
5. Write the release-readiness report.

## Task-plan contract

The master specification is implemented by ordered horizontal task files named
`2026-07-13-unified-cooperative-runtime-task-NN.md`. Each task must contain:

- status, dependencies, immutable input contracts, and exact start state;
- a complete file/module inventory for its layer;
- agent-executable steps and allowed parallel work boundaries;
- temporary adapters with owner and deletion task;
- tests, stress cases, source scans, and exact verification commands;
- durable evidence and independent review report paths;
- finding-to-regression-test remediation instructions;
- explicit completion, commit, and handoff criteria.

Tasks are not sized for manual review convenience. They are sized to complete a
horizontal architectural layer without leaving its ownership model ambiguous.
An implementation agent does not approve its own task.

## Task 01 status

Task 01 was partially executed in commit `52293e61` before this specification
resolved ownership and fairness semantics. It is **not accepted**.

The revised task must address these known defects in its own plan and evidence:

- `race_with_settled_winner_cancels_owned_pending_loser` contradicts the public
  observational `async/race` contract. Replace it with a test proving a supplied
  loser continues. The owned-loser case belongs to the concurrency-layer task
  that introduces `async/race-owned`; it is not a valid current-runtime
  characterization oracle.
- Existing earlier tests that expect `async/all` or `async/race` to cancel
  supplied siblings must also be inventoried and corrected rather than left as
  contradictory oracles.
- `ready_spinner_does_not_starve_due_timer` currently terminates only because of
  the legacy one-million-tick failure. Once that ceiling is deleted, an unfair
  runtime would hang in-process forever. Move the fairness oracle behind an
  external watchdog or a deterministic bounded drive harness before removing
  the ceiling.
- The scratch report at
  `/tmp/unified-runtime-task-01-implementer-report.md` must be replaced by
  durable evidence under the plan evidence directory.

The useful characterization tests for captured mutation, duration validation,
capacity validation, finite yield count, and nested callback composition remain
inputs to the corrected task.

## Release gates

The final branch passes the repository CI-equivalent suite:

```text
cargo test --workspace
jake examples
jake smoke-bytecode
jake lint
jake docs-check
```

It also passes every deterministic scheduler seed, real-resource stress suite,
WASM browser suite, cancellation/leak/shutdown suite, static removal scan,
generated-asset check, packaged web-boundary test, and review gate defined by
the task plans.

Every command runs from a documented clean state. Expected RED tests are allowed
only during intermediate layers and are enumerated in that layer's evidence;
none remain at release readiness.

## Definition of done

The rewrite is complete only when:

- every Sema execution and suspension path uses the interpreter-owned runtime;
- multiple roots interleave fairly with isolated root context and documented
  shared-global semantics;
- observational and owned concurrency APIs match this specification;
- every resource has a tested cancellation class and bounded shutdown behavior;
- every task-local field has a tested inheritance policy;
- the migration inventory contains no unreviewed production site;
- all legacy scheduler, re-entry, polling, and browser replay paths are deleted;
- all correctness, stress, leak, host, WASM, docs, asset, and CI gates pass;
- every independent review finding is fixed and verified or explicitly disposed
  when it is not a correctness/safety issue;
- final profiling is complete, regressions are understood, any tuning has been
  reverified, and confirmation benchmarks are recorded;
- the final release-readiness report finds no unresolved correctness, safety,
  resource, determinism, or maintainability blocker.
