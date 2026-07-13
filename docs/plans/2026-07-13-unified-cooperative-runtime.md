# Unified Cooperative Runtime Rewrite

## Purpose

Replace Sema's split, re-entrant async machinery with one interpreter-owned
cooperative runtime. This is a direct replacement for the next release: there
is no compatibility scheduler and no feature flag preserving the old path.

The rewrite exists because async execution is currently spread across VM
yield signals, a scheduler that temporarily removes running tasks, thread-local
callbacks, synchronous callback re-entry, subsystem-specific `IoHandle`
polling, and host-specific event loops. That arrangement makes nested async
composition, cancellation, fairness, captured mutation, resource ownership,
and WASM behavior depend on which route happened to enter the evaluator.

The completed design has one rule: every unit of Sema execution is a runtime
task, and every operation that cannot finish immediately suspends that task in
the same runtime. Native functions, callbacks, I/O, timers, channels, child
tasks, servers, LLM tools, MCP calls, REPL evaluation, notebooks, and WASM all
obey the same rule.

## Non-negotiable decisions

- Hard cut. Delete the legacy scheduler in the same release.
- The interpreter owns exactly one runtime. The VM remains the sole evaluator.
- A root `eval` is a task; nested evaluation never starts a second scheduler.
- Tasks have stable IDs and remain in the task table while running.
- Scheduling is FIFO with a 10,000-instruction quantum.
- External completions are data placed into an inbox. Worker threads never
  execute Sema, mutate VM state, or call evaluator callbacks.
- Native functions return `Return`, `Call`, or `Suspend`; resumable natives use
  continuation frames instead of re-entering evaluation.
- Captured variables are shared cells. Parent and child tasks observe the same
  mutation; task snapshots must not clone cell state.
- Cancellation is structured and owned-only. Cancelling an aggregate cancels
  children it created, never arbitrary promises passed into it.
- `race` resolves on the first settlement, including errors and cancellation.
- Pending children may outlive an evaluation root and remain owned by the
  interpreter until awaited, cancelled, or interpreter shutdown.
- Task-local evaluation, sandbox, tracing, usage, LLM, file/module, debugger,
  and resource context is explicit. Scheduler correctness must not depend on
  thread-local ambient state.
- WASM `eval()` returns a Promise. `evalAsync()` may be retained as an alias.
- No synchronous XHR replay, replay markers, `Atomics.wait`, or host-side
  scheduler loop remains in browser execution.

## Target architecture

### Runtime ownership

`Interpreter` owns a `Runtime`. Hosts submit roots through the interpreter and
drive that same runtime until the requested root settles or until the host must
yield to its event loop. A nested call only manipulates the active task's VM
stack.

### Runtime state

The runtime contains:

- `TaskTable<TaskId, Task>` with monotonic stable IDs;
- FIFO `ready` queue with a per-task queued bit to prevent duplicate enqueue;
- timer min-heap ordered by deadline and insertion sequence;
- typed external-completion inbox and waiter index;
- promise, channel, resource, and child-wait registries;
- root handles that let hosts observe settlement without owning scheduling;
- deterministic virtual-clock support for tests;
- shutdown state that cancels tasks, deregisters waits, and reaps resources.

Task states are explicit: `Ready`, `Running`, `Waiting(WaitKey)`, and
`Completed(Result<Value, SemaError>)`. State transitions are checked centrally.
A task never disappears merely because it is executing.

### Drive loop

One turn performs these steps in order:

1. Drain external completions and wake their exact waiters.
2. Expire every due timer and enqueue its exact waiters.
3. Run one ready task for at most 10,000 VM instructions or until it returns,
   errors, suspends, or is cancelled.
4. Apply the resulting transition and enqueue newly ready tasks in FIFO order.
5. Re-check completions and timers before another ready task so a perpetual
   ready workload cannot starve I/O or time.
6. If nothing is ready, return the next wake deadline/inbox wait requirement to
   the host instead of blocking inside the evaluator.

### Native suspension protocol

Native calls have a runtime-aware result:

```text
NativeOutcome = Return(Value)
              | Call { callable, args, continuation }
              | Suspend { wait_key, continuation }
```

`NativeContinuation` is stored on the task's VM/native frame stack. When a
wait completes, the runtime writes a typed completion into that frame and
requeues the task. Higher-order functions and tool/server callbacks use
`Call`; they do not invoke a global evaluator callback synchronously.

### Waits and completions

Every suspension registers a `WaitKey` owned by the runtime. Completion payloads
are typed and one-shot. Cancellation removes the waiter and invokes the wait's
cancel/reap hook. Late completions are harmless because delivery validates the
wait generation and task state.

Required wait families include promises/tasks, timers, channel send/receive,
worker-pool jobs, processes, PTYs, streams/files, sockets/HTTP/WebSocket,
databases/KV/git/archive/PDF/serial/secret operations, server requests, LLM
requests/streams/tools, MCP connection queues, and host/WASM futures.

### Structured ownership

Each task records a parent and owned children. `async/spawn`, `async/all`,
`async/map`, pool operations, and `race` record which children they create.
Aggregate completion or cancellation only propagates to those owned children.
Explicitly supplied promises are observed but not adopted. Interpreter shutdown
cancels all remaining roots and descendants, then drains cancellation/reaping.

### Shared cells and task locals

Lexical bindings captured across task boundaries point at the same traceable
cell. Spawning copies references to cells, not values or an environment
snapshot. Task-local context is copied according to a documented inheritance
policy and restored from the task record on every resume; no task-local value
leaks between siblings.

## Complete migration inventory

The inventory is a release gate, not a suggestion. Each production site below
must either use the unified runtime or be documented as synchronously incapable
of suspension. The implementation must keep
`docs/internals/async-runtime-inventory.md` current as sites move.

### Core and VM

- `crates/sema-core/src/async_signal.rs`
- `crates/sema-core/src/io_backend.rs`
- `crates/sema-core/src/context.rs`
- `crates/sema-core/src/lib.rs`
- `crates/sema-vm/src/scheduler.rs`
- `crates/sema-vm/src/vm.rs`
- `crates/sema-vm/src/debug.rs`
- `crates/sema-eval/src/eval.rs`
- `crates/sema-eval/src/prelude.rs`

### Async primitives and callback users

- `crates/sema-stdlib/src/async_ops.rs`
- `crates/sema-stdlib/src/context.rs`
- `crates/sema-stdlib/src/list.rs`
- `crates/sema-stdlib/src/map.rs`
- `crates/sema-stdlib/src/meta.rs`
- `crates/sema-stdlib/src/string.rs`
- `crates/sema-stdlib/src/system.rs`
- `crates/sema-stdlib/src/typed_array.rs`
- `crates/sema-stdlib/src/workflow.rs`
- `crates/sema-stdlib/src/otel.rs`

### I/O, resources, processes, and servers

- `archive.rs`, `diff.rs`, `event.rs`, `git.rs`, `http.rs`, `io.rs`, `kv.rs`,
  `pdf.rs`, `proc.rs`, `pty.rs`, `secret.rs`, `serial.rs`, `server.rs`,
  `sqlite.rs`, `stream.rs`, `terminal.rs`, and `ws.rs` in
  `crates/sema-stdlib/src/`.
- Resource close/drop/cancel paths and the shared I/O backend in `sema-core`.

### LLM, MCP, and agents

- `crates/sema-llm/src/builtins.rs` and all tool/agent callback paths.
- `crates/sema-mcp/src/builtins.rs` and `crates/sema-mcp/src/tools.rs`.
- Provider streaming, retry, timeout, budget, tracing, and usage state.
- Per-connection MCP serialization without global scheduler serialization.

### Host entry points

- `crates/sema/src/lib.rs`, `main.rs`, CLI file/eval/build execution, and REPL.
- DAP, LSP, notebook engine, MCP server tools, embedded library callers.
- `crates/sema-wasm/src/lib.rs`, playground workers/client, shipped web runtime.

### Tests, docs, examples, and generated surfaces

- Every async integration test in `crates/sema/tests/`.
- `examples/async-everything.sema`, all async examples and playground samples.
- Concurrency, evaluator, architecture, bytecode VM, and web-server docs.
- Builtin docs entries and generated index.

## Implementation sequence

1. Add characterization tests that fail on known defects and stress invariants.
2. Introduce runtime/task/wait types behind the interpreter without migrating
   language behavior yet.
3. Make root evaluation and VM quantum preemption use the new task table.
4. Add native continuation frames and migrate higher-order callbacks.
5. Migrate promises, scopes, cancellation, timers, channels, and shared cells.
6. Migrate every blocking/offloaded stdlib leaf to typed completion delivery.
7. Migrate process, PTY, stream, socket, HTTP, WebSocket, and server lifecycles.
8. Migrate LLM, agent tool loops, streaming, and MCP queues.
9. Migrate every host; make browser evaluation genuinely asynchronous.
10. Delete `IoHandle`, `IoPoll`, `YieldReason`, scheduler TLS callbacks,
    re-entrant scheduling, replay/marker/XHR/Atomics paths, and compatibility
    branches.
11. Add static inventory checks and complete documentation/conformance updates.
12. Pound the runtime with deterministic and real-clock stress workloads, then
    run the complete release verification suite.

## Required behavioral tests

- Parent observes child mutation through a shared captured cell.
- Nested callback/native chains may spawn, await, sleep, perform I/O, and error.
- Nested `all`/`race`/map/pool combinations cannot hide or lose the parent task.
- `race` first settlement wins and only owned losers are cancelled, including
  the pre-settled fast path.
- Cancelling each wait family wakes/reaps exactly once; late completion is safe.
- Process stdin write and process wait can overlap without deadlock.
- Bounded-channel FIFO/backpressure/close races are correct; impossible or huge
  capacities produce a Sema error without allocation panic.
- Negative, NaN, infinite, and overflowing durations are rejected before
  rounding; sub-millisecond valid durations behave consistently.
- More than one million yields completes without a global tick ceiling.
- Ready storms cannot starve timers or external completions.
- Stable task lookup remains correct under heavy spawn/complete/cancel churn.
- Task-local file/module/sandbox/tracing/usage/LLM context never leaks.
- Server requests overlap up to configured limits and disconnect cancellation
  reaches the exact handler task.
- LLM streams and MCP queues make progress concurrently and clean up on cancel.
- Pending tasks survive eval-root completion and can be awaited later.
- Interpreter drop cancels/reaps all pending operations without leaks or panic.
- WASM Promise evaluation supports timers, fetch, channels, cancellation,
  nested callbacks, and debugger stepping without replay.

## Release gates

No legacy symbol or host workaround may remain outside explicit migration tests.
CI searches production code for the removed APIs and fails on reintroduction.
The final branch must pass:

```text
cargo test --workspace
jake examples
jake smoke-bytecode
jake lint
jake docs-check
```

It must also pass deterministic scheduler stress seeds, native real-clock I/O
stress, WASM browser tests, leak/teardown tests, and the packaged web boundary
test when shipped runtime assets change. Every discovered failure gets a small
reproducer committed before its fix.

## Definition of done

All Sema execution and suspension routes through the interpreter-owned runtime;
the inventory contains no unmigrated production site; legacy scheduling and
browser replay code is deleted; all required behavioral and release gates pass;
and a final branch-wide review finds no unresolved correctness issue.
