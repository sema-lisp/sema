# Fable 5 re-architecture strategy — kill the TLS bridge, finish the structural ABI

Authored by the Fable 5 model (2026-07-14) after reading the plan doc + all load-bearing
code. This is the migration reference for the async-layer re-architecture. The owner's
mandate: migrate 100% to the canonical cooperative scheduler with NO legacy bridges and NO
thread-locals; rewrite tests to check BEHAVIOUR (not that old mechanisms work 1-for-1);
delete tests that test the wrong thing and write correct ones; do not invent behaviour not
in the plan.

## Key discovery
The runtime already contains a complete, working consumer for the structural ABI —
`apply_native_outcome`, `install_protocol_suspend`, `dispatch_runtime`, `invoke_callable`,
`resume_continuation`, and `ReturnOwner::VmResume`/`reinstall_parent_vm` in
`crates/sema-vm/src/runtime/state.rs` already handle
`NativeOutcome::{Return,Call,Suspend,Runtime}` end-to-end. The TLS bridge exists only
because the **VM's native dispatch never speaks the ABI** — it calls `(native.func)(ctx,
args)` and polls `take_yield_signal()`. The pivot is surgical.

## Ground truth (verified in code)
- `sema-core/src/runtime/native.rs`: `NativeOutcome = Return | Call | Suspend | Runtime`;
  `WaitKind = Timer | Promise(PromiseId) | PromiseSet | Channel | External`;
  `RuntimeRequest` already has Spawn, CancelPromise, CreateChannel, ChannelOp,
  CreateSettledPromise, InspectPromise, PromiseSetWait, OriginBarrier. ~80% unconsumed.
- `NativeFn::invoke_runtime(eval_ctx, &mut NativeCallContext, args) -> NativeResult` with a
  legacy fallback (`func -> NativeOutcome::Return`). Production calls it in exactly ONE
  place: `state.rs` `invoke_callable` (the HOF callback path). The VM never calls it.
- VM has FOUR native dispatch sites that poll TLS: `CALL`/`TAIL_CALL` opcode arms (via
  `call_value`), `CALL_NATIVE`, and `CALL_GLOBAL` native arm (via `call_native_with`). All
  return `VmExecResult::AsyncYield(YieldReason)`.
- `run_parked_quantum` (state.rs:1352) translates `YieldReason` -> `TaskAction::{VmSleep,
  VmSpawn,VmCancel,VmAwait,VmAwaitSet,VmChannel*,VmAwaitIo}`. The whole match is the bridge.
  Its `NativeYield` arm already proves the target shape (packs parked VM into
  `ReturnOwner::VmResume`, dispatches the `NativeOutcome`).
- Consumer gaps: `install_protocol_suspend` rejects `WaitKind::Timer` (unreachable) and
  `PromiseSetMode::Timeout`; `dispatch_runtime` Spawn returns "runtime spawn admission is
  unavailable" (real spawning lives only in the YieldReason path `spawn_detached`).
- Two promise mechanisms: `spawned_promises: HashMap<TaskId, Rc<AsyncPromise>>` +
  `promise_waits` (state.rs:180-183) mirror settlements into the legacy `AsyncPromise`,
  while the checked `PromiseRegistry` serves only the protocol path. Legacy stores rejection
  as `String`; the registry's `TaskSettlement` preserves the real `SemaError`.

## Step sequence (ordered; only Step D is an accepted red window)

- **Step A — runtime consumer gaps** (state.rs only): Timer suspend, Timeout promise-set,
  `dispatch_runtime` Spawn/CancelPromise/OriginBarrier, finish
  CreateSettledPromise/InspectPromise/ChannelOp. Tree green; runtime unit tests.
- **Step B — VM structural ABI, additive**: add `VmExecResult::Pending(VmPendingOutcome)`;
  make all 4 dispatch sites call `invoke_runtime` and propagate structurally; add
  `run_quantum` cancellation param; add the `Pending` arm in `run_parked_quantum` while
  KEEPING the `AsyncYield` arm temporarily so unconverted ops still work. Tree green.
- **Step C — pilot op**: convert `async/sleep` alone (Timer wait). Proves full pipeline.
- **Step D — ATOMIC (red window)**: `AsyncPromise{id}`/`Channel{id}` thin handles +
  registry-only settlement (delete `spawned_promises`/`promise_waits`) + convert ALL
  remaining async_ops per the table below. Update printers/ValueView/cycle tracing here too.
- **Step E — HOF cleanup**: HOFs return `NativeOutcome::Call` directly; delete
  `YieldReason::NativeYield`, `PENDING_NATIVE_OUTCOME`, `set/take_pending_native_outcome`,
  the yield-signal pickup in `invoke_callable`.
- **Step F — External I/O sites**: convert every `IoHandle`/`AwaitIo` producer to
  `WaitKind::External` on the `ThreadPoolExecutor`. Delete `LegacyAwaitIo`, `IoHandle`,
  `IoPoll`, `poll_io_waits`, `io_park`/`notify_io_complete`. Parallelizable per-module.
- **Step G — legacy callback re-entry**: migrate remaining `call_callback` fresh-VM users to
  `NativeOutcome::Call`; delete `suspend_runtime_quantum`/`QuantumSuspendGuard` and the
  fresh-VM-entry bridge. Triage `sort` comparator / `json/encode` toJSON interleaved
  callbacks (restructure as CPS/continuation-driven).
- **Step H — the purge + guard**: delete `scheduler.rs`, `YieldReason`,
  `set/take_yield_signal`, `RESUME_VALUE`, `IN_ASYNC_CONTEXT`, `IN_RUNTIME_QUANTUM`,
  `SchedulerTarget/SchedulerRunResult/DebugCoopResume`, dead `TaskAction::Vm*`,
  `VmExecResult::AsyncYield`. Add the static-removal source-scan test.
- **Step I — test sweep + verify**: rewrite mechanism tests as behaviour tests (cite plan
  contract rows), delete wrong tests, run `jake examples`/`jake smoke-bytecode`/full suite.

## async_ops.rs conversion table (target NativeOutcome)
| Op | Returns |
|---|---|
| `async/spawn` | `Runtime(Spawn{callable, cont})` -> cont gets `Promise(id)` -> `Return(promise value)` |
| `async/await` | `Suspend{wait: Promise(id), cont}`; cont maps Returned->value, Failed->preserved SemaError, Cancelled->structured `:cancelled` |
| `async/all` / `race` | `Suspend{wait: PromiseSet{All\|Race}}` |
| `async/timeout` | `Suspend{wait: PromiseSet{Timeout(d)}}` |
| `async/sleep` | `Suspend{wait: Timer(d), cont: ReturnNil}` |
| `async/cancel` | `Runtime(CancelPromise)` -> `Cancelled(bool)` |
| `async/resolved` / `rejected` | `Runtime(CreateSettledPromise{outcome})` -> `Promise(id)` |
| `resolved?`/`rejected?`/`pending?`/`cancelled?` | `Runtime(InspectPromise)` -> `Settlement(Option)` -> bool |
| `async/run` | `Runtime(OriginBarrier)` — replaces the non-plan-conformant `Sleep(0)` hack |
| `channel/new` | `Runtime(CreateChannel{capacity})` (keep arg validation) |
| `channel/send` / `recv` | `Suspend{wait: Channel(Send/Receive)}`; cont maps Sent->nil, Closed->error/nil-sentinel per contract |
| `channel/close`, `try-recv`, `count/empty?/full?/closed?` | `Runtime(ChannelOp{Close \| TryReceive \| Inspect(q)})` |

## VM structural ABI shapes (Step B)
```rust
// debug.rs
pub enum VmExecResult {
    Finished(Value), Stopped(StopInfo), Yielded,
    QuantumExpired { instructions: usize },
    Pending(VmPendingOutcome),   // replaces AsyncYield
}
pub enum VmPendingOutcome { Call(NativeCall), Suspend(NativeSuspend), Runtime(RuntimeRequest) }
```
- VM dispatch: build `NativeCallContext { task_context: ctx.task_context(), cancellation }`,
  call `native.invoke_runtime(...)`. `Return(v)` -> push, continue. Otherwise push a nil
  placeholder, park pc past the call, return `VmExecResult::Pending(outcome.into_pending())`.
- `run_quantum(&context, limit, cancellation: CancellationView)` stores cancellation in a VM
  field for the quantum's lifetime (no TLS). Runtime builds it from
  `task.record.cancellation()` in `run_parked_quantum`.
- `run_parked_quantum` Pending arm == existing `NativeYield` arm minus the TLS pickup:
  ```rust
  Ok(VmExecResult::Pending(pending)) => {
      let parent = task.vm_owner.take().expect("VM call has a return owner");
      let owner = ReturnOwner::VmResume { vm: Box::new(vm), parent: Box::new(parent) };
      TaskAction::VmResult(task_id, owner, Ok(pending.into_outcome()))
  }
  ```

## Promise/channel reconciliation (Step D)
```rust
// value.rs
pub struct AsyncPromise { pub id: sema_core::runtime::PromiseId }
pub struct Channel      { pub id: sema_core::runtime::ChannelId }
```
- Delete `value.rs::PromiseState`, `task_id: Cell<u64>`, Sema-side channel buffer,
  `LegacyRuntimeBridge`. `PromiseId`/`ChannelId` are `Copy`, carry `RuntimeId` (the checked
  identity — foreign/dead runtime -> structured error).
- Delete `spawned_promises`/`promise_waits`; add a reverse `task -> PromiseId` index inside
  `PromiseRegistry` so `settle_task` finds the promise. `spawn_detached` allocates via
  `promises.allocate_pending(Some(child))`.
- GC: settled Values traced through `PromiseRegistry::trace`. Value-side handle holds no
  Values -> its trace edges vanish; delete the double-trace in `RuntimeState::trace`.

## Tests
- KEEP (behavioural): nearly all of `vm_async_test.rs`; runtime/tests.rs behaviour cases.
- REWRITE: tests asserting messages/paths the plan changes — string-prefixed
  `"task rejected:"`, synchronous `"channel is full"`, `"still pending after scheduler run"`,
  `async/run` drain-only semantics (now an origin-root barrier). Assert on structured
  condition fields, not strings.
- DELETE: mechanism tests — `async_signal.rs` TLS unit tests, `scheduler.rs` tests,
  `LegacyRuntimeBridge` tests, IoHandle-shaped assertions, YieldReason/spawned_promises cases.
- ORACLE / anti-invention rule: the plan's "Language-facing concurrency contract" tables are
  the ONLY source of expected behaviour. Every rewritten assertion must cite the plan
  row/sentence it encodes; if the plan doesn't specify it, don't assert it.

## Risks / traps
1. RefCell re-entrancy on `TaskContext`: borrow per-native-call only, never across the
   quantum. Keep `suspend_runtime_quantum` alive until Step G (legacy `call_callback`
   re-entry).
2. Missing a dispatch site -> nil placeholder returned as real result. During B-D add a debug
   assertion that no TLS signal is set after every native return; after H the type system
   enforces it.
3. CORE-2 GC: every new continuation struct needs a trace test. The promise re-shape reduces
   risk (continuations capture `PromiseId` Copy, no edges). Verify `ReturnOwner::VmResume`
   trace covers the boxed parked VM.
4. Non-`Send` boundary: `PreparedExternalOperation` constructors enforce no Value/Rc in the
   job — don't weaken them.
5. Call vs Suspend inside one native (LLM tool loop): continuation chain handles it — each
   resume may return another Call or Suspend. Budget time for sema-llm's agent loop.
6. Observational ops must resolve synchronously into `pending` (dispatch_runtime does) — no
   wait registered.
7. `async/run` OriginBarrier waits transitively (vs the weaker `Sleep(0)` drain) — a
   plan-conformant behaviour change; rewrite affected tests deliberately.

**Bottom line**: finish 3 consumer gaps (A), make the VM speak `NativeOutcome` structurally
via the existing `invoke_runtime` (B), rewrite `async_ops.rs` onto the existing
`RuntimeRequest`/`WaitKind` vocabulary with thin `PromiseId`/`ChannelId` handles (C-D), then
delete the bridges outward (E-H) with a static source-scan gate. Only Step D is atomic.
