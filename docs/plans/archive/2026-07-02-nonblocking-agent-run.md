# Non-blocking multi-round `agent/run` (issue #61 §3a / cooperative-scheduling M1)

**Status:** design approved (adversarial doc-review panel, 2026-07-02) → implementing.
Supersedes the "yield-internally in one native" sketch in
`docs/plans/2026-07-01-cooperative-scheduling.md` §3a. ADR #68.

## Problem

A single `llm/complete` already offloads its wire round-trip onto the shared
runtime and yields `YieldReason::AwaitIo`, so concurrent completions interleave.
But **`agent/run` (and `llm/chat` with tools) does not**: both drive
`run_tool_loop` (`crates/sema-llm/src/builtins.rs`), a blocking `for _round in
0..max_rounds` that calls the *synchronous* `do_complete`. The whole multi-round
conversation runs inside one native call, so every sibling scheduler task is
frozen for its entire duration — and `async/timeout`/`async/cancel` cannot
interrupt it. "If async is broken, agents are broken."

## Why the loop must move to bytecode (the pivotal finding)

A native that yields `AwaitIo` is **not re-invoked** on resume — the scheduler
resumes the bytecode *after* the CALL via `replace_stack_top`, and the poller
(run inside `Scheduler::wake_blocked_tasks`, holding `&mut self.tasks`) produces
the value. Two consequences, both verified by code-tracing:

1. **A poller cannot arm a second `AwaitIo`** — the yield signal is only consumed
   at VM CALL sites, so a `set_yield_signal` from inside a poller runs outside any
   VM and is dropped. So one native call yields at most once; it cannot
   loop-yield-loop-yield across rounds.
2. **Tools cannot execute inside a poller.** During `wake_blocked_tasks` the
   scheduler has been *taken out* of its thread-local (`take_scheduler`), so any
   re-entry (`run_closure_as_inline_task`) returns a clean `Err` (no UB, but no
   scheduler); a synchronous tool would run on the paused VM, but an **async tool**
   (one that calls `llm/complete`, `await`, channel ops) hard-errors "async yield
   outside of scheduler context" or silently degrades to the blocking path —
   re-freezing siblings, the exact regression we set out to remove.

Therefore the round loop **must live in bytecode** (a task the top-level scheduler
drives), calling a native that does one offloaded round and yields. Tools then run
in ordinary task context where the VM-closure fallback routes correctly through
`run_closure_as_inline_task` **with** the scheduler present.

## Architecture (approved)

A thin **Sema/prelude driver loop** over four internal natives, coordinated by a
Rust-owned opaque **`AgentRun` handle**. The blocking `run_tool_loop` is **kept
byte-identical** for the synchronous (top-level, non-`async/spawn`) and `wasm32`
paths — the new driver is *additive*, reached only when `in_async_context()`.

### The handle — `AgentRunState`

An `Rc<RefCell<AgentRunState>>` wrapped as an opaque, non-printable `Value` handle,
**stamped with the owning scheduler `task_id`** (captured in `__agent-begin`). It
owns everything that cannot thread as a plain Sema value or that must survive a
park:

- `Vec<ChatMessage>` — history + tool-call correlation, **never leaves Rust**.
- tool `ToolSchema`s + the tool `Value`s + `on_tool_call` / `on_text` closures.
- model / max_tokens / temperature / system / reasoning_effort.
- `round` and `consecutive_errors` counters; `MAX_TURNS`, `MAX_CONSEC_ERRORS = 5`.
- `first_input` (for the trace-I/O rollup).
- the conversation/session/user scope guard(s) and the **agent OTel span**
  (started attached; ended once in `__agent-finish`), plus an `ended: bool` flag.
- (M-budget, separate PR) a captured `Rc<RefCell<BudgetFrame>>` budget snapshot.

**Borrow discipline (tested invariant):** no `__agent-*` native may hold a
`RefCell::borrow_mut()` across `call_callback` / `execute_tool_call` / any
inline-task spin. Each native short-borrows to copy owned inputs out, drops the
borrow, does the yielding/callback work, then short-borrows again to write back.
`Drop` uses `try_borrow_mut` + the `ended` flag so an unwind never double-panics.

### Span lifetime (the fix the panel forced)

`__agent-begin` starts the agent span the **normal attached way** (`start()` pushes
its context onto the thread-local span STACK, becoming the top). The **existing
per-task otel swap** in `ReinstallGuard` (`install_task_otel`/`restore_otel`) saves
and restores the agent task's whole stack across every inter-round / inter-tool
park — so the agent span is the live parent whenever `__agent-step` or
`__agent-exec-tools` runs, and each round's **detached** chat span (from
`do_complete_async_yield`'s `llm_span_detached`, parent captured synchronously
before the yield) parents under it. `__agent-finish` does the **balanced** pop+end
while the agent task's otel is installed — never a blind off-stack drop (which
`SpanCore::drop` would mis-pop, corrupting a sibling's trace).

### The four natives

- **`__agent-begin(agent, input, opts) → handle`** — ports today's `agent/run`
  setup: opts/session/memory parse, initial message assembly, `build_tool_schemas`,
  conversation-id + scope install, start the attached agent span, apply call
  telemetry, stamp `owning_task_id`. Returns the handle.
- **`__agent-step(handle) → {:done bool :content str}`** — short-borrow: build the
  `ChatRequest` from `handle.messages`, snapshot budget/usage, drop borrow. If
  `on_text` present → synchronous `do_complete_streaming` (on-text is validated
  synchronous-only). Else → `do_complete_async_yield` (the single per-round
  `AwaitIo` yield) whose finalize (poller, VM thread) re-borrows to push the
  assistant turn, records content, and flags `done` when there are no tool calls or
  the round/consec bounds are hit. All per-round accounting (track_usage-once,
  cache, cassette, per-leaf usage, serving-provider) is inherited unchanged.
- **`__agent-exec-tools(handle) → nil`** — runs in ordinary async task context so
  yielding/async tools suspend correctly. Short-borrow to copy the pending
  tool-call list, drop borrow, then per call: fire `on_tool_call` start,
  `execute_tool_call` (may spin a nested inline task), tool span, `on_tool_call`
  end; re-borrow to push the correlated `ChatMessage::tool_result` (id+name) and
  update `consecutive_errors`.
- **`__agent-finish(handle) → result`** — balanced span end + `set_trace_io` +
  memory writeback + build the output (`{:response … :messages …}` /
  `Conversation`); set `ended`. **Idempotent** and invoked from three triggers:
  normal loop exit, a Sema `finally`, and the handle's `Drop` on task cancel
  (Sema `finally` does *not* run when a task is cancelled).

### Sema driver (prelude)

```sema
(define (__agent-drive h)
  (if (:done (__agent-step h))
      (__agent-finish h)
      (begin (__agent-exec-tools h) (__agent-drive h))))
```

`agent/run` and `llm/chat`-with-tools become thin dispatchers: in async context,
`(let ((h (__agent-begin …))) (try (__agent-drive h) (finally (__agent-finish h))))`;
otherwise the existing blocking native. Loop bounds (max-turns, consec-error abort)
are enforced **in the Rust handle** (checked in `__agent-step`/`__agent-exec-tools`),
so `:done` fires deterministically regardless of the Sema loop.

## Acceptance gate (written first, RED today)

`crates/sema/tests/agent_async_test.rs` (+ `FakeProvider::tool_loop`, a
request-keyed multi-round script deterministic under any interleaving):

1. `concurrent_agents_overlap_and_peak_inflight` — N agents, peak in-flight ≥ 2,
   wall ≈ max not sum.
2. `sibling_ticker_advances_during_agent_rounds` — a sibling advances *during* the
   rounds (snapshot > 0).
3. `cancelling_agent_run_cuts_the_loop_short` — `async/timeout` cuts the loop
   (calls < full; **never assert an exact cutoff** — the in-flight `spawn_blocking`
   round always completes on the worker and is discarded).

Plus (Step 6): a `FakeRecorder` round-2 correlation assertion under interleaving,
a consecutive-tool-error-abort assertion, an OTel sibling-non-contamination test
(cancel one of two concurrent agents; the survivor's spans stay intact), and a
sub-agent-reentrancy test (a tool that itself calls `agent/run`).

## Plan (each step independently committable, tree green)

- **Step 0 — RED gate** (done): oracle + `FakeProvider::tool_loop`, `#[ignore]`d.
- **Step 1 — `AgentRunState` handle** + opaque `Value`, `owning_task_id`, borrow
  discipline, idempotent `Drop`.
- **Step 2 — `__agent-begin`** (port setup, attached span).
- **Step 3 — `__agent-step`** (`do_complete_async_yield` reuse; sync/stream branch).
- **Step 4 — `__agent-exec-tools`** (task-context tools, correlation).
- **Step 5 — `__agent-finish`** (idempotent; Drop-on-cancel) + prelude driver +
  `agent/run`/`llm/chat` dispatch on `in_async_context()`.
- **Step 6 — green gate**: un-ignore the oracle, add correlation / consec-error /
  sibling-isolation / sub-agent-reentrancy tests. Full CI-equivalent.
- **Step 7 (SEPARATE PR) — ASYNC-1 budget-across-yield**: enforce the poller's
  `track_usage` against a handle-captured `Rc<RefCell<BudgetFrame>>` snapshot
  (mirroring `usage_accum_slot`), not a `ReinstallGuard` swap (which is inactive
  during `wake_blocked_tasks`). Pre-existing single-completion gap; decoupled.
- **Step 8 — docs**: rewrite M1 to this model; ADR #68 supersedes #8.

## Honest limits (documented, not silent)

- ~~Streaming rounds block siblings~~ **CLOSED 2026-07-03**: streaming got the
  same lift-the-loop treatment. In async context `llm/stream` and agent `:on-text`
  rounds run the wire side (the provider's synchronous SSE drive) on the I/O pool,
  sending deltas over a channel; the bytecode `__stream-drive` loop parks on
  `AwaitIo` between delta batches (the poller drains all currently-available
  deltas per wake, amortizing park/resume over fast token streams) and calls the
  callback per delta IN TASK CONTEXT — siblings interleave between deltas, and a
  callback that itself yields (`async/sleep`, channel ops, `await`) is now
  supported. Pinned by `stream_async_test.rs` (sibling-ticker-during-stream
  oracle, yielding-callback, usage-once, ordering, mid-stream-error). The
  remaining truth: **the callback itself runs synchronously per delta on the VM
  thread** — a CPU-bound `:on-text` callback still holds the thread between
  yields. Sync/top-level `llm/stream` keeps the byte-identical blocking native;
  a cancelled task parked in `__stream-next` abandons its slab entry and the
  wire worker streams to completion into a dead channel (best-effort, same as
  completion offloads).
- **Synchronous CPU-bound tools between rounds block siblings** (no preemption of
  Sema code — the standing single-threaded limit).
- ~~In-flight-round cancel is best-effort~~ **CLOSED 2026-07-03**: the wire stage
  is an `io_spawn`ed future (`run_fallback_retry_async` over per-provider
  `complete_future` hooks) with a real `AbortHook` in `IoHandle::with_abort` — a
  cancelled agent's current round is dropped mid-flight (connection torn down),
  like the http/shell tier. Pinned by `llm_request_is_aborted_on_timeout`
  (true_cancel_test.rs). Best-effort remains only for sync-only providers (the
  `complete_future` default impl, e.g. FakeProvider), pinned by
  `sync_only_provider_cancel_is_best_effort`.
- ~~Cancelled agents leak their slab entry (and never-ended agent span) until
  `reset_runtime_state`~~ **CLOSED 2026-07-03**: the scheduler fires a
  `task-reaped` callback (`sema-core` seam) at every cancellation transition, and
  `sema-llm`'s registered sweep removes every `AGENT_RUNS` entry stamped with the
  reaped task's id — ending the agent span balanced on the VM thread. Pinned by
  `cancelled_agent_leaves_no_slab_entry_and_next_run_works` /
  `cancelled_agent_span_is_exported` (agent_async_test.rs).
- ~~Budget under concurrent spawned agents under-enforces~~ **CLOSED 2026-07-03**:
  ASYNC-1's per-task LLM scope capture (merged from main) composes with the
  offload's dispatch-time budget-frame `Rc` snapshot — enforcement crosses
  spawn+yield. Pinned by `budget_enforced_across_spawn_and_yield`
  (complete_async_test.rs); Step 7 is done, no separate PR needed.
