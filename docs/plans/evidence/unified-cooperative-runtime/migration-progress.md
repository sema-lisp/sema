# Structural-ABI migration — progress log

Live log of the async re-architecture (killing the thread-local YieldReason bridge, moving
100% to the canonical cooperative Runtime + structural NativeOutcome ABI). Sequenced from
Fable 5's strategy (see fable5-rearchitecture-strategy.md), adjusted as real code forced
reorderings. Each committed gate is evidence-verified (goal-skill: the gate is the truth).

## Committed gates

- **Step A** (`feat(runtime): fill structural-ABI consumer gaps`): Timer suspend, registry
  Spawn admission (spawn_via_registry + task_promises + settle_registry_child), promise-set
  Timeout. Additive, behind the kept YieldReason bridge. 4 runtime unit tests. sema-vm 490/0.

- **Steps B+C** (`feat(vm): route native dispatch through structural NativeOutcome ABI`):
  VM native dispatch calls NativeFn::invoke_runtime under a runtime quantum and propagates
  non-Return outcomes as VmExecResult::Pending(VmPendingOutcome) — no TLS hop. Gated on the
  per-EvalContext runtime_quantum_active() Cell (NOT a new thread-local); synchronous
  re-entry clears it so nested natives keep the value ABI (fixes the RefCell re-borrow trap).
  run_quantum threads a CancellationView. async/sleep converted to
  Suspend(WaitKind::Timer) as the pilot — proves park→VmResume→timer→continuation→resume.
  Legacy AsyncYield path kept intact for un-converted ops. vm_async at its (then) 4 reds.

- **Step D0** (`feat(eval): make the unified runtime the sole async engine`): flipped
  eval_str_compiled from the legacy run_exprs_on_vm (init_scheduler + VM::execute) to
  run_exprs_via_runtime. This made the unified Runtime the sole async engine for ALL real
  execution (CLI file/-e, MCP eval, notebook, REPL, tests). run_exprs_on_vm deleted as dead
  code. The runtime path was strictly BETTER than legacy (the 4 legacy reds now pass); only
  3 legacy-mechanism reds surfaced: concurrent_sleeps rewritten to behaviour; vm_eval nested
  async → deferred Step G; event_select cooperative yield → deferred Step F.
  eval 1072/0, integration 1055/0, vm_async 117/0 (1 defer), vm_integration 147/0 (1 defer).

- **Step D1** (`feat(runtime): drive .semac/bytecode execution on the unified runtime`):
  extracted Interpreter::drive_vm_on_runtime (the submit_root + drive loop) and routed the
  two pre-compiled-bytecode runners (CLI .semac, MCP run_file) through it, removing the last
  non-debug init_scheduler calls. .semac async now works on the runtime (prints 42). DAP +
  wasm init_scheduler remain (deferred debug backends).

## Key discovery (reordered the plan)

The promise VALUE reshape (AsyncPromise → {id}) is NOT separable from decommissioning the
legacy scheduler. The public `AsyncPromise{state, task_id}` is the legacy scheduler's
COMPLETION CELL: scheduler.rs constructs it (~30 sites) and reads/writes `.state`; sema-core's
`SchedulerTarget`/`DebugCoopResume`/`YieldReason::{AwaitPromise,AwaitPromiseSet,Cancel}` carry
`Rc<AsyncPromise>` by pointer identity; and the legacy scheduler cannot mint a `PromiseId`
(no runtime). So "reshape AsyncPromise" and "leave scheduler.rs alone" were mutually
exclusive. Verified: `run_closure_as_inline_task` (the scheduler's completion path) is reached
only under `in_async_context()` (vm.rs:733), which the runtime quantum path never sets — so
the legacy scheduler is now debug/legacy-IO-callback only; core runtime async uses
NativeOutcome::Call.

**Unblock (in progress, Step D2):** split a scheduler-private `LegacyPromise` type (the current
`{state, task_id}` completion cell) out of the public `AsyncPromise`, repointing scheduler.rs +
vm.rs debug helpers + async_signal enums to it. This frees the public `AsyncPromise` to become
the canonical `{id}` handle, keeps scheduler.rs compiling, and confines the reshape to the
async ops + runtime + cycle.rs. DAP/wasm async-DEBUG (which can't run runtime-only ops) is
deferred via #[ignore] + a runtime cooperative-debug-mode future item; sync debugging is
unaffected.

## Remaining sequence

- **D2** (in progress): LegacyPromise split + AsyncPromise{id} reshape + convert 12 promise
  ops to structural + delete runtime legacy promise bridge (spawned_promises etc.) +
  OriginBarrier (async/run) + cycle.rs simplification + rewrite "task rejected" string tests
  to behaviour + #[ignore] DAP/wasm async-debug.
- **D3**: channels — Channel{id} reshape + convert channel ops to structural + delete legacy
  channel bridge.
- **E**: HOF cleanup — HOFs return NativeOutcome::Call directly; delete YieldReason::NativeYield,
  PENDING_NATIVE_OUTCOME, set/take_pending_native_outcome.
- **F**: External I/O — convert IoHandle/AwaitIo producers (sema-llm, sema-mcp, stdlib
  http/io/proc/git/sqlite/kv/stream/pty/ws/serial/system/list) to WaitKind::External on the
  ThreadPoolExecutor. Restores event_select deferral. Parallelizable per-module.
- **G**: legacy callback re-entry — migrate remaining call_callback fresh-VM users to
  NativeOutcome::Call; delete suspend_runtime_quantum. Restores nested-eval-async deferral.
- **H**: the purge — delete scheduler.rs, YieldReason, set/take_yield_signal, RESUME_VALUE,
  IN_ASYNC_CONTEXT, IN_RUNTIME_QUANTUM, LegacyPromise, DebugCoopResume/SchedulerTarget,
  VmExecResult::AsyncYield, dead TaskAction::Vm* + the static-removal source-scan gate. This
  is where the DAP/wasm cooperative-debug-mode question must be resolved (build it, or make
  the async-debug deferral permanent+documented).
- **I**: test sweep + full verification (jake examples, jake smoke-bytecode, workspace suite).
