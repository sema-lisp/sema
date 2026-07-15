# Unified Cooperative Runtime — Remaining-Work Implementation Plan

**Branch:** `codex/unified-async-runtime` · **Author:** Fable 5 (senior-architect pass, 2026-07-15)
**Status:** DRAFT — pending 2 independent Opus verifier sanity-checks before implementation.
**Scope:** everything after the completed language-async migration (structural `NativeOutcome`
ABI, `PromiseRegistry`/`ChannelRegistry`, `WaitKind::External` one-shots, agent re-entry — all
green). Covers phases P0–P8 and resolves the deferred blockers in `docs/deferred.md`
(F2-RESIDUAL-1/2/3, ASYNC-DEBUG-1, ASYNC-RUN-BARRIER-1, ASYNC-TIMEOUT-CANCEL-1, LEGACY-SCHEDULER,
inventory reconciliation).

---

## 0. Executive summary and phase graph

```
P0  Executor async tier (reactor)          ──┐  gates P1, P2 (abort fidelity + concurrency)
P1  Resource-queue primitive + checkout I/O ─┤  gates P5 (AwaitIo deletion)
P2  Streaming/remaining I/O off AwaitIo    ──┤
P3  DEBUG ON THE RUNTIME (B — the crux)    ──┤  gates P5 (scheduler.rs deletion)   [parallel with P0–P2]
P4  Task-context generalization (Task 06)  ──┤  soft-gates P5 (IN_ASYNC_CONTEXT deletion touches llm/otel sites)
P5  THE PURGE (Task 08 deletable core) + inventory reconciliation (G)
P6  Hosts (Task 07): common host API, wasm Promise roots, playground, services, SRV-1
P7  Verification campaign (Task 09) + six-round review (Task 10)
P8  Profiling / benchmarking / release readiness (Task 11)
```

Two explicit ordering answers:
1. **The executor async-tier fix (P0) goes first.** Every subsequent I/O conversion inherits its
   concurrency model + abort semantics; converting onto the blocking workaround and re-plumbing
   later touches every module twice. P0 also fixes the latent `sema-llm` `interruptible_async`
   panic (`crates/sema-llm/src/builtins.rs:7209`, masked because only FakeProvider exercises it).
2. **The debug redesign (P3) hard-gates the purge (P5)** — `scheduler.rs` now exists *solely* as
   the debug backend (`init_scheduler` callers: `sema-dap/src/server.rs:998`,
   `sema-wasm/src/lib.rs:2111`); `LegacyPromise`/`LegacyChannel`/`DebugCoopResume`/`SchedulerTarget`
   exist solely to serve it. P3 is independent of P0–P2 (different code regions) — run in parallel.

---

## 1. P0 — Executor async tier: a real reactor behind the seam

**Problem (verified).** `ThreadPoolExecutor::run_dispatch` (`crates/sema-vm/src/runtime/host.rs:330-339`)
polls `ExecutorDispatch::Async` with a bare thread-parking `block_on` (host.rs:361) — no tokio
reactor, so any real tokio/reqwest future panics. Consequences: (a) every migrated External op uses
`interruptible_blocking` + `io_block_on` + a `tokio::select!` cancel race
(`crates/sema-stdlib/src/runtime_offload.rs:195-210`), consuming **one pool worker per in-flight op**
(pool clamped [2,8] — a hard I/O concurrency ceiling); (b) the sema-llm runtime completion op is a
landmine on real network.

**Design.** Implement the Task-05-specified production executor in `sema-io` (which owns the
process-wide multi-thread tokio runtime, `crates/sema-io/src/lib.rs:59-98`):
- `ProcessIoExecutor: IoExecutor` + `ProcessExecutorLease: ExecutorLease` (shapes fixed in Task 05
  spec lines 114-157 — implement those, don't invent a second seam).
- **Async tier:** `ExecutorDispatch::Async` → `tokio::spawn` on the shared runtime; drop/cancel =
  `AbortHandle::abort` (true drop-on-cancel, no worker burned). `io_spawn` (lib.rs:137-143) already
  returns an `AbortHook`.
- **Blocking tier:** `spawn_blocking` under the existing `OFFLOAD_SEM` admission (448 permits).
  Keep `ExecutorSnapshot` counters + bounded lease `shutdown(deadline)` exactly as `ThreadPoolExecutor`
  (host.rs:188-196, 236-260); port its four regression tests.
- Wire `build_runtime` (`crates/sema-eval/src/eval.rs:86-94`) to construct `ProcessIoExecutor` as
  `Arc<dyn IoExecutor>` (avoid a direct sema-eval→tokio edge; expose a `sema_io::process_executor()`
  factory if needed). Keep `ThreadPoolExecutor` for wasm/no-io builds.
- Add `runtime_offload::external_io_async` (+ `_try`) mirroring `external_io_interruptible`
  (runtime_offload.rs:225-292) but building `interruptible_async`; convert **http** to it as the
  reference (transport-tier swap, not a semantic change).
- Fix/validate the sema-llm site `do_complete_runtime_suspend` (builtins.rs:7161-7229) — now runs.
  Gate with a FakeProvider concurrency test PLUS one recorded live smoke (AGENTS.md LLM flow).

**Gates.** `cargo test -p sema-io`; new `runtime_external_async_test`: (1) N=16 `http/get` overlap
~1× wall-time; (2) `async/cancel` of a parked `http/get` aborts promptly (<100 ms); (3) llm
FakeProvider overlap green; live `llm/complete` smoke.
**Effort: S–M.** Mechanical; seam types exist + tested.

---

## 2. P1 — Per-handle availability primitive + checkout-op migration (F2-RESIDUAL-1)

**Ground truth.** Six modules share a verbatim-duplicated checkout state machine
(`Available/CheckedOut/Tombstone` + `Acquire→Running(oneshot)` poller + `AwaitIo` + `io_spawn_blocking`):
`proc.rs` (registry proc.rs:80), `sqlite.rs` (sqlite.rs:70), `kv.rs` (kv.rs:73), `serial.rs`
(serial.rs:79), `pty.rs` (pty.rs:57), `stream.rs` (slot on the stream object, stream.rs:509-592).
All resources `Send`-asserted. `sema-mcp` is the proven migrated template
(`crates/sema-mcp/src/builtins.rs:1609,1639`) — but models busy as a poll+retry
(`McpAcquireContinuation`).

**C3 decision — acquire-queue shape. RECOMMEND: a first-class `ResourceGate` runtime component**
(not MCP-style poll-retry; the Task-05/08 scans forbid polling in a runtime task path):
- `sema-core/src/runtime/`: `ResourceGateId` (runtime-scoped, like `ChannelId`); a new
  `WaitKind::ResourceSlot(ResourceGateId)`.
- `sema-vm/src/runtime/`: a `ResourceGateRegistry` mirroring `ChannelRegistry` (~200 lines + tests):
  per-gate `busy` + FIFO `WaitKey` queue; `acquire` immediate-or-park; `release` wakes head; cancel
  of a parked waiter removes it (wire into `cancel_waiting`, state.rs:802); gate-close fails waiters.
- `runtime_offload::checkout_external(gate, slot, job, decoder, cancel_hook)` — ONE shared helper:
  acquire gate (suspend on `ResourceSlot` if busy) → checkout `Send` resource → `interruptible_blocking`
  (or `_async`) → decoder checks in + releases on the VM thread → cancel tombstones + releases.
  Collapses the six duplicated machines.

**Conversion order** (independent after the helper — parallelizable): `sqlite` → `kv` → `proc`
(process-group cancel hook, reuse shell's pattern) → `pty` → `serial` → `stream` → migrate the
`fs_offload` opens onto plain `external_io_*`/quarantined → re-point MCP onto the gate (deletes its
poll loop). Also convert remaining sema-llm `AwaitIo` sites (embeddings builtins.rs:3438-3491/…,
completion-async 7081-7142, stream-delta poller 10036, `event/select`, `io/read-key-timeout`).

**Gates.** Per-module async tests + new cancellation cases (cancel-while-queued, cancel-mid-job,
FIFO fairness, tombstone-after-cancel); restored `event_select_yields_to_sibling_in_async_context`;
grep: zero `set_yield_signal` producers outside still-pending modules.
**Effort: L (2–4 sessions, fan-out).**

---

## 3. P2 — Streaming ops (F2-RESIDUAL-2): no new streaming primitive needed

**Finding:** of the three "streaming" modules, `pty`+`stream` are checkout-shaped one-shots (P1
covers them). Only `ws` is a true pump (persistent `io_spawn` pump bridging socket↔mpsc, ws.rs:307-403;
`ws/recv` polls `try_recv()` on an `Rc<RefCell<mpsc::Receiver>>` via AwaitIo, ws.rs:439-514).

**Decision: do NOT add a streaming External-wait shape.** Restructure `ws`: receiver endpoint
(`mpsc::Receiver` — Send) in a checkout slot; `ws/recv` = P1 `checkout_external` with an async-tier
job `rx.recv().await` (timeout variant via `tokio::time::timeout`); decoder returns the receiver.
`ws/send`/`ping` stay synchronous. `ws/connect` handshake → `external_io_async` with `abort_pump`
cancel hook. Each language-level `recv` is one suspension; backpressure is the channel bound. Closes
F2-RESIDUAL-2 with zero new runtime surface — document as a plan amendment (a streaming primitive is
NOT needed). Sweep remaining small AwaitIo/offload users (`terminal`/`event`/`secret`/`archive`/`pdf`/
`diff`/`git`/`system`) per the Task 05 matrix; produce `task-05-resource-matrix.md`.

**AwaitIo funeral (exit for P1+P2):** zero `YieldReason::AwaitIo` producers → delete in state.rs
`io_waits`/`poll_io_waits`/`await_io`/`TaskAction::VmAwaitIo`/`legacy_io_wakeup_required` + the
`io_park` arm in `drive_vm_on_runtime` (eval.rs:352-367); in sema-core `IoHandle`/`IoPoll`/`io_park`/
`notify_io_complete` (async_signal.rs:92-164,811-834). Severable from P5 — its own commit.
**Effort: M.**

---

## 4. P3 — THE DEBUG ARCHITECTURE (B): decision, design, staging

### 4.1 Decision
**A runtime debug quantum + a paused-task barrier, reusing the existing VM debug interpreter and the
existing `ACTIVE_DEBUG` session registration — not a new stepping engine, not the legacy thread-local
model.** This is a DIFFERENT approach from Task 07's implied new `DebugCoordinator`: the coordinator
shrinks to ~3 fields of paused-state bookkeeping in `RuntimeState`, because the hard parts exist on
both sides of the seam:
- VM already has a full debug interpreter: `run_inner::<true>(ctx, Some(debug))` with breakpoints,
  conditions, step modes, exception stops (`vm.rs:2205`), and a shared blocking stop-server
  `handle_debug_stop` (vm.rs:1489) whose inspection commands target the running VM (= the paused task's).
- Every task runs through one choke point: `run_parked_quantum` (state.rs:1227) calls
  `vm.run_quantum(...)` = `run_inner::<false>` with NO `RefCell` borrow held across the quantum
  (1234-1237 / 1278) — a quantum may block or return a stop without deadlocking the state cell.
- `ACTIVE_DEBUG` TLS stack + `is_debug_session_active()`/`with_active_debug` (vm.rs:822-995) is the
  proven mechanism to reach a `DebugState` that can't be handed through a seam — reuse unchanged.

Rejected: a from-scratch DAP-aware runtime debugger (cross-task stepping etc.) — ASYNC-2 keeps
cross-task stepping out of scope; shipped bar is STOP+CONTINUE+inspect+within-task stepping; build
the minimum that is architecturally FINAL (runtime-owned pause state, no legacy types).

### 4.2 Design (concretely)
**(a) Debug quantum.** `VM::run_quantum_debug(ctx, limit, cancellation, debug) -> VmQuantumResult` =
`run_quantum` but `run_inner::<true>(ctx, Some(debug))`. In `run_parked_quantum`, when
`is_debug_session_active()`, run the debug variant via `with_active_debug` (as `step_task_debug` does,
scheduler.rs:909-927). Thread the same through `invoke_callable`/`resume_continuation` callback-VM
runs (state.rs:2106,2243) so breakpoints in cooperative HOF callbacks fire — this UPGRADES the legacy
behavior (the wasm auto-continue HOF-callback hack, scheduler.rs:613-655, disappears).

**(b) Two stop protocols, one primitive**, by `DebugState::is_headless()` (debug.rs:250):
- **Blocking (native DAP):** on `Stopped(info)` in the debug quantum, call
  `vm.handle_debug_stop(ctx, debug, info)` right there inside the quantum (no state borrow held; VM
  thread parks on `command_rx` serving inspection against the stopped task's VM). This IS the plan's
  stop-the-world barrier (nothing else runs on the interpreter thread; completions queue, timers
  don't fire). On Resume, loop back into `run_inner::<true>`. Drive turns only see terminal/suspend —
  small DAP diff.
- **Cooperative (wasm/headless):** on `Stopped`, park the task (`vm_call=Some(vm)`, don't re-enqueue),
  record `debug_paused: Option<(RootId, TaskId, StopInfo)>` in `RuntimeState`, return
  `TaskAction::DebugStop`. `drive` surfaces `DriveState::DebugStopped{root,task,info}`. While
  `debug_paused` set, `drive` runs no ready task + fires no timer (completions accepted, not delivered).
  Resume: host sets step mode, `Runtime::debug_resume()` clears the barrier, re-enqueues the paused
  task at the FRONT of its root's queue, drives.

**(c) Inspection.** `Runtime::with_paused_task_vm<R>(f: impl FnOnce(&mut VM)->R) -> Option<R>` reaches
`tasks[debug_paused.task].vm_call`. Replaces `scheduler::with_coop_paused_task_vm` (533),
`COOP_TASK_STOP`/`set_coop_task_stop`/`DebugCoopResume`/`reconstruct_coop_resume_value` (vm.rs:840-955)
wholesale — the paused task's VM never leaves the runtime; resume is ordinary quantum re-entry.

**(d) Step semantics across the scheduler.** `stepping_task: Option<TaskId>` beside `debug_paused`.
The debug quantum applies step-mode stop logic (`should_stop`, debug.rs:266-273) ONLY when running
the stepping task; breakpoints/pause apply to all. Stepping task suspends over an `await` → siblings
run (breakpoints only), step re-arms when it next runs (ASYNC-2 contract: step stays within the task;
sibling-crossing stepping stays deferred). `step_frame_depth` from the stepped VM at resume (as wasm,
lib.rs:3140-3149).

**(e) Host rewiring.**
- **DAP** (server.rs:993-1000): replace `init_scheduler`+`execute_debug` with `ActiveDebugGuard::enter(ds)`
  → build VM as today → `interpreter.drive_vm_on_runtime(vm)` (eval.rs:283). `execute_debug`'s outer
  concerns (entry stop, `break_on_uncaught` exception park via `debug_exception_park` vm.rs:1603,
  post-Terminated drain) move to the DAP backend around the drive call. Async ops under the debugger
  Just Work. Closes ASYNC-DEBUG-1's 2 DAP tests.
- **wasm** (lib.rs:2038-2246): submit root on `self.inner` with headless `DebugState`; enter/exit the
  ACTIVE_DEBUG guard around each `drive` call (re-entered between JS turns via the stored session);
  `debugPoll`/`debug_resume` = bounded drive turns mapping `DebugStopped→stopped`; `GetLocals`/
  `GetStackTrace` via `with_paused_task_vm` on the paused (root) task. Closes the 7 wasm ASYNC-DEBUG-1
  tests + `playground/tests/async-debugger.spec.ts`.

**(f) What this kills (deletable in P5):** `scheduler.rs`, `LegacyPromise`, `LegacyChannel`,
`SchedulerTarget`, `SchedulerRunResult`, `DebugCoopResume`, `COOP_TASK_STOP`, `run_cooperative`/
`start_cooperative`/`execute_debug`/`execute_async*`/`run_async*`, `VmExecResult::AsyncYield` consumers.

### 4.3 Staging / effort / risk
- **B1 (native DAP, blocking):** debug quantum + DAP rewire + un-ignore 2 DAP tests. Riskiest point:
  blocking inside a drive turn — audit no `RefCell`/`WaitRuntime` borrow live across the quantum
  (verified main path; re-verify `invoke_callable` callback quanta). 1–2 sessions.
- **B2 (headless barrier + wasm):** `TaskAction::DebugStop`, `debug_paused`/`stepping_task`,
  `DriveState::DebugStopped`, `with_paused_task_vm`, wasm rewire, un-ignore 7 wasm tests + playwright. 1–2.
- **B3 (semantics):** step-task gating, multi-root barrier assert, cancel/Stop while paused (wasm
  `debugStop` clears barrier + cancels root), exception stop, conditional breakpoints (evaluate on the
  paused VM — verify nested eval under the quantum; explicit test). 1.
- **Design gap to watch:** `DebugState` host-owned + consulted via TLS pointer (same aliasing
  discipline as legacy `ACTIVE_DEBUG`, vm.rs:989-992) — inherited risk; type-safe alt (runtime owns
  `Rc<RefCell<DebugState>>`) is a clean follow-up if review objects, no architecture change.

**Total P3: 3–5 sessions. Confidence high** — every load-bearing mechanism exists + is tested; the
work relocates orchestration from `scheduler.rs` to `runtime/state.rs`.

---

## 5. P4 — Task-context generalization (D / Task 06 remainder)

**Verified gap (LIVE CORRECTNESS ISSUE):** `RuntimeTask` carries ONLY `llm_scope` (state.rs:113,
captured at spawn 2401, swapped per quantum 1265-1276). The legacy scheduler swapped THREE scopes —
`otel`, `usage_scope`, `llm_scope` (scheduler.rs:67-81,1200-1217). The runtime never picked up
otel/usage → two interleaving tasks share the thread-local OTel span stack + leaf-usage scope →
span corruption + usage misattribution under interleaving is live NOW (no green test forces a
mid-span task interleave). **P4 step 1 is a FAILING TEST:** two spawned tasks open spans / accrue
usage in forced alternation; assert stack balance + per-leaf attribution.

Then right-sized: (1) generalize the swap seam — replace `llm_scope: Box<dyn Any>` with the Task-02
`TaskContextHandle` extension map (`HashMap<TypeId, Rc<dyn TaskLocalValue>>`, already built):
`LlmTaskState`/`OtelTaskState`/`UsageTaskState`, each with explicit `inherit()`, captured at
`spawn_via_registry`, installed around the quantum via one panic-safe `TaskContextGuard`. (2)
Workflow/MCP context (`WorkflowTaskState`/`McpTaskState`) + `mcp/tools`/`mcp/close` external-wait
migration. (3) Context/TLS matrix (Task 06 Task 1) → `task-06-context-matrix.md`.
Gates: Task 06 suites. Sequence P4's guard refactor AFTER B1 or coordinate on `run_parked_quantum`.
**Effort: M–L (2–3 sessions).**

---

## 6. P5 — The purge (Task 08 core) + inventory reconciliation (G)

Preconditions: P1+P2 (AwaitIo gone), P3 (debug off scheduler.rs), P4 recommended.
Delete (verify caller-free first; one subsystem per commit + focused suites):
1. `scheduler.rs` entirely; `init_scheduler`/`shutdown_scheduler`/`reset_scheduler_tasks`/
   `scheduler_task_count` (lib.rs:35); `Interpreter::drop`'s `shutdown_scheduler` (eval.rs:109).
2. `vm.rs`: `execute_debug`, `run_cooperative`, `start_cooperative`, `execute_async(_debug)`,
   `run_async(_debug)`, `COOP_TASK_STOP`, `surface_coop_task_stop`, `reconstruct_coop_resume_value`;
   `VmExecResult::AsyncYield` + its `run_parked_quantum` arm (state.rs:1290-1329) + dead `TaskAction::Vm*`.
   (`handle_debug_stop` STAYS — used by the debug quantum.)
3. `async_signal.rs`: `YieldReason`, `set/take_yield_signal`, `RESUME_VALUE`, `LegacyPromise`/
   `LegacyChannel`/`PromiseState`, `SchedulerTarget`/`SchedulerRunResult`/`DebugCoopResume`, scheduler
   callback seams, `IN_ASYNC_CONTEXT` (+ the ~130 `in_async_context()` branches — per-site decision:
   runtime-only ops drop the gate; keep `in_runtime_quantum` until P6). Likely delete the whole module.
4. **Static gate flip:** extend `scripts/check-unified-runtime-legacy.sh --check` to zero-tolerance
   with the Task 08 fixture list; delete `legacy-symbols.baseline`; exact file+symbol+proof+owner
   allowlist, no globs.
5. **Inventory reconciliation (G) — automate:** regenerate discovery matches (Task-01 regex) vs
   current source; join to `runtime-match-map.tsv` on `(file, symbol)` NOT line numbers; auto-disposition
   (a) deleted→closed-by-deletion, (b) unchanged+valid→carry, (c) new/moved→hand-review (small). Ship as
   a checked-in classifier feeding `unified_runtime_inventory_mapping_covers_exact_current_matches`. 1 session.
6. Fix `llm_fake_test::agent_turn_boundary` GC threshold (re-derive, don't blind-lower).
7. Docs/examples/assets sweep DEFERRED to P6-tail (needs final host behavior + wasm asset regen).
**Effort: M (1–2 sessions).**

---

## 7. P6 — Hosts (Task 07)
1. **Common host API** (`submit_value/submit_str/drive/cancel_root/command_handle/shutdown` on
   `Interpreter`, `RootOptions`, root-tagged `OutputEvent`, `RuntimeCommandHandle` as the only `Send`
   surface). Much exists privately (`drive_vm_on_runtime`, `RootHandle`, `ShutdownOptions/Report`).
   Ctrl-C via `RuntimeCommandHandle::cancel_root` replaces `check_interrupt` polling.
2. **DAP/LSP/notebook/MCP-server/workflow hosts** — plumbing onto (1); DAP already rebuilt in P3.
3. **WASM Promise-driven roots** (the big one): `eval()` returns a Promise; macrotask drive turns;
   fetch/timers as external completions; delete `HTTP_AWAIT_MARKER`/`MAX_REPLAYS` replay, Atomics sleep,
   `installAtomicsSleep`, `set_blocking_sleep_callback`. Single-threaded browser executor whose async
   tier is JS-callback completions. Playwright gates.
4. **SRV-1 (`http/serve` handler-task-per-connection)** — needs owned scopes + External accept/request
   waits; nothing in P5 depends on it. Largest single stdlib redesign left; own failing-test matrix.
5. Deferred Task-08 tail: docs/examples/notebooks, `jake wasm.*` regen, `scripts/test-packaged-sema-web.sh`,
   `jake docs-check` — AGENTS.md shipping invariant as review checklist.
**Effort: XL. Parallelizable: (2),(3),(4) disjoint after (1).**

---

## 8. P7/P8 — Verification, review, release
- **P7a (Task 09):** adversarial matrix (cancellation at every boundary; duplicate/stale/wrong-generation
  completions; multi-root orderings; >1M yields; channel races; resource fault injection; task-local
  leakage; GC cycle stress; wasm heartbeat), seeded-random scheduling + injectable virtual `RuntimeClock`
  (verify it landed — former flip-blocker; else a P7 prerequisite), leak plateaus, watchdogged hang
  detection, Windows-native watchdog CI leg.
- **P7b (Task 10):** six independent review rounds; the 2 Opus verifiers of THIS plan are round 0.
- **P8 (Task 11):** profile vs the pinned baseline (`3f111e83` — confirm SHA), benchmark surface,
  tune-reverify, release-readiness report. No profiling before P7 green.
**Effort: L–XL, remediation-dominated.**

---

## 9. Owner decisions (C)
**C1 — `async/run` (ASYNC-RUN-BARRIER-1). RECOMMEND: amend to a "self-resolving-waits barrier"** —
provably deadlock-free and strictly stronger than the drain: `(async/run)` suspends while any other
origin-root task is Ready/Running/timer-parked/External-parked (all self-resolving), re-evaluated
continuously; releases when the residual origin-root graph is settled or parked ONLY on intra-runtime
promise/channel waits (the only cycle-forming kinds). Transitivity is automatic (a settling sleeper's
awaiter becomes Ready; the barrier keeps waiting). Fixes the repro (`(async/spawn (fn () (async/sleep
30) (println "bg"))) (async/run)` prints "bg") without touching the two hazard cases (self-awaited
parent / rendezvous-blocked child are promise/channel-parked → excluded). Implementation: a real
`OriginBarrier` wait re-checked in `drive` on every origin-root settlement/park transition. ~1 session.
Fallback: amend the contract to the drain (zero risk).

**C2 — eager cancellation delivery to External waits (ASYNC-TIMEOUT-CANCEL-1). RECOMMEND: deliver at
request time.** When a cancellation is recorded on an External/IO-parked task, synchronously run the
wait teardown then (deregister, invoke executor cancel/resource hook once, enqueue cancelled
settlement); count pending-cancel teardown as progress in `drive_vm_on_runtime`'s post-settle drain so
a one-shot `-e` flushes aborts; `Interpreter::drop` bounded shutdown is the backstop. Observational
`async/timeout` semantics unchanged (a supplied promise's task legitimately continues). ~1 session
inside P1. **Also fix UCR-3 (R3) here** (rendezvous cancel dropping a committed value, state.rs:818-825).

**C3 — acquire-queue primitive: RECOMMEND `ResourceGate`** (see P1) over MCP poll-retry.
**C4 — Task 06 right-sizing: RECOMMEND** typed `TaskContextHandle` extensions for LLM/OTel/usage/
workflow/MCP + one guard, NOT four separate `task_context.rs` skeletons — amend Task 06's file list.

---

## 10. Risk register
| # | Risk | Class | Mitigation |
|---|---|---|---|
| R1 | OTel/usage per-task isolation is a LIVE correctness gap now (only llm_scope swapped) | Genuine, maybe user-visible | P4 step 1 failing test; consider hotfix of the two extra swaps ahead of the full refactor |
| R2 | Blocking `handle_debug_stop` inside a drive turn deadlocks if a state borrow/extraction is live across a quantum | Design (P3) | Audited for `run_parked_quantum`; debug_assert state-not-borrowed at quantum entry; re-audit callback quanta |
| R3 | UCR-3: channel-rendezvous cancel can drop a committed value (state.rs:818-825) | Known latent | Fix in C2's pass (don't select a rendezvous-matched wait) |
| R4 | ProcessIoExecutor shutdown vs multi-interpreter tests | Mechanical | Port the four lease-lifetime regression tests; lease-scoped shutdown only |
| R5 | Deleting IN_ASYNC_CONTEXT flips ~130 stdlib sites; a missed one silently changes top-level behavior | Mechanical, wide | Per-site decision table; `jake examples`+`smoke-bytecode` (81) as the net |
| R6 | wasm Promise migration is the largest unknown; browser executor is genuinely new | Design (bounded by master §Browser/WASM) | Land host API + native hosts first; Playwright heartbeat as oracle |
| R7 | sema-llm async-tier path has never run against a real provider | Verification | P0 gate includes recorded live smoke |
| R8 | Conditional-breakpoint Evaluate re-enters eval from inside a paused quantum | P3 edge | Explicit B3 test; synchronous re-entry, expected fine |
| R9 | Inventory reconciliation balloons if hand-done | Process | Automate (P5 §5); hand-review only bucket (c) |
| R10 | C1 barrier: an External-parked descendant that never completes now blocks `async/run` | Semantics trade-off | External waits interruptible post-P0/P1; Ctrl-C; document; acceptable |

---

## 11. Sequencing / effort (scoping)
| Phase | Content | Effort (sessions) | Parallel with |
|---|---|---|---|
| P0 | Executor async tier + llm fix | 1 | P3 |
| P1 | ResourceGate + 6 checkout modules + llm AwaitIo + C2 + C3 | 2–4 (fan-out) | P3 |
| P2 | ws + small-module sweep + AwaitIo deletion | 1–2 | P3 |
| P3 | Debug on the runtime (B1→B2→B3) | 3–5 | P0–P2 |
| P4 | TaskContext (otel/usage first) + workflow/MCP + C4 | 2–3 | after B1 |
| P5 | Purge + zero-legacy gate + inventory (G) + C1 barrier | 2 | — |
| P6 | Host API, services, wasm Promise, playground, SRV-1, docs/assets tail | 6–9 | internal fan-out |
| P7 | Task 09 campaign + Task 10 six rounds | 4–8 (remediation) | — |
| P8 | Task 11 profiling/release | 2–3 | — |

**Critical path: P3 → P5 → P6 → P7 → P8.** P0–P2 fit inside P3's shadow. Safely severable for descope:
SRV-1 (ship fail-fast guard) and C1's barrier (ship the drain with amended contract); everything else
is load-bearing for the purge or release gates.
