# Structural-ABI migration — progress log

## C1-DONE (2026-07-16): `async/run` self-resolving-waits barrier (ASYNC-RUN-BARRIER-1)

Replaced the `async/run` ready-DRAIN (a zero-duration `Timer` suspension) with a real
self-resolving-waits barrier. `RuntimeRequest::OriginBarrier` now parks the caller on
`ProtocolWaitKind::OriginBarrier { root }`; `Runtime::resolve_origin_barriers` (called at the
top of every drive iteration) resumes it once no OTHER origin-root task is Ready/Running or
parked on a self-resolving wait (`Timer` / `External` / `Timeout`-mode `PromiseSet`). Cycle-
forming waits (`Promise`, all·race `PromiseSet`, `Channel`, `ResourceSlot`, nested barrier) are
excluded — `ResourceSlot` is cycle-forming per the Reviewer-2 hole (a held slot's holder may be
excluded, so waiting on the waiter would hang). Repro now prints `bg` before `after-run`;
transitivity is automatic via per-iteration re-check. Tests: 4 out-of-process wall-clock-guarded
Sema tests in `vm_async_test.rs` + `async_run_barrier_releases_over_resource_slot_cycle`
(drive-turn-bounded ResourceSlot cycle) in `runtime/tests.rs`. Deferred entry → RESOLVED.

## MILESTONE (2026-07-16): the legacy scheduler is DELETED (P5 — the purge)

The core goal is achieved at the code level. The unified cooperative `Runtime` is the SOLE
execution engine; there are no legacy bridges. P0–P5 of the remaining-work plan
(docs/plans/2026-07-15-remaining-work-plan.md), all committed + verified by an exhaustive
per-binary `cargo test --workspace` sweep (green except the one inventory-mapping deferral):
- **P-hotfix**: per-task OTel/usage isolation (a live regression).
- **P0**: ProcessIoExecutor real reactor — concurrency ceiling gone (16 http/get in 306ms), the
  latent sema-llm real-network panic fixed.
- **P1**: ResourceGate primitive + all 6 checkout-I/O modules (sqlite/kv/proc/pty/serial/stream)
  onto WaitKind::ResourceSlot/External; mcp/call gate; event/select; C2 eager cancel + UCR-3.
- **P3-B1 + P3-B2**: debug runs on the unified runtime for BOTH native DAP and wasm (the
  purge-unblocking crux, R2-verified deadlock-free). B3 (cross-sibling step gating) remains as
  refinement below the shipped feature bar.
- **P2**: the AwaitIo funeral — the runtime is 100% off the IoHandle thread-local bridge; all
  I/O flows through WaitKind::External. (sema-llm + ws converted; a real LLM-cancel abort fixed.)
- **P5 (the purge)**: ~9,500 lines deleted — scheduler.rs, LegacyPromise/LegacyChannel, IoHandle,
  SchedulerTarget/DebugCoopResume, IN_ASYNC_CONTEXT, the promise/channel/AwaitIo YieldReason
  variants, and the ~130 dead in_async_context legacy branches. A zero-tolerance static-removal
  gate prevents reintroduction.

**Remaining (completeness / hosts / verification, NOT the core transformation):** P4 (Task-06
TaskContext generalization — the otel/usage/llm swap is already live; workflow/tracing/mcp typed
scopes remain); P6 (Task-07 hosts: common host API + wasm Promise-driven roots + SRV-1 http/serve
+ docs/examples/wasm-asset regen + package-boundary gate — the largest remaining, and where the
wasm build is verified); P7 (Task-09 adversarial/fuzz/leak campaign + Task-10 six-round review);
P8 (Task-11 profiling/benchmark/release-readiness). Plus residuals: YieldReason::Sleep cleanup
(likely dead), the inventory-mapping per-site reconciliation (436 residual), and P3-B3.

**Every phase now ends with the exhaustive per-binary sweep** (the corrected discipline after the
mid-migration regression-recovery episode below).


## REGRESSION RECOVERY (2026-07-15, after an inadequate first verification)

The initial "migration complete, full suite green" claim was WRONG: verification ran a subset of
test binaries + `jake examples`/`smoke-bytecode`, not an exhaustive per-binary `cargo test
--workspace` sweep, so ~19 binaries / ~35 tests (all green at the pre-session baseline ee3a7aa9)
were regressed and undetected. Recovered across five root-cause clusters, each committed:
- **async overlap**: llm/chat/complete/embed/batch/rerank, archive/pdf/diff, stream/file, http/serve
  gated offload on the dead `in_async_context()` → ran synchronously; extended the guard fix.
- **callback re-entry**: `otel/span`(+variants), `workflow/run`/`step` → NativeOutcome::Call.
- **GC leak (real memory bug)**: a cycle routed through a channel-registry buffer OR a
  settled-promise value was invisible to the CORE-2 residual collector → never reclaimed;
  reintroduced GcNode::Channel/Promise with I2-preserving registry-interior hooks + dead-handle
  eviction. Promises had the identical latent leak — fixed symmetrically.
- **cooperative agent/chat tool loop**: ran only synchronously at the root (a prior shortcut);
  now cooperative at all levels with journaling ported into the continuation + a detached tool span.
- **cancellation**: transitive `async/cancel` via the cancellation-parent graph + eager subprocess
  abort + a runtime-accurate `live_task_count()` oracle + cancelled-agent span export.
Plus stragglers: rewrote a retired lambda-wrap-hint test, documented the owned combinators
(`async/race-owned`/`async/with-timeout`), reconciled the legacy-symbols conformance baseline.

**STANDING GATE (corrected discipline):** every phase now ends with an exhaustive
`cargo test --workspace --no-fail-fast` per-binary sweep + `jake examples`/`smoke-bytecode`/`lint`,
not a hand-picked subset. Re-established green baseline: full workspace green except the single
documented deferral (`unified_runtime_inventory_mapping` — the ~1000-site disposition re-review).


## FINAL STATE (2026-07-15): language async 100% migrated, full suite green

The thread-local suspension bridge is DELETED for all language-level async. Every unit of
execution runs on the ONE interpreter-owned cooperative `Runtime`; suspension goes through the
structural `NativeOutcome` ABI. Verified: `jake examples` 81/0, `jake smoke-bytecode` 81/0,
`jake lint` clean, `cargo test --workspace` green (sema-vm 490/0, sema-core 317/0, eval 1072/0,
integration 1055/0, complete_async 14/0, stream_async 10/0, llm_fake 29/0, all async/IO suites
green). The ONLY remaining reds are documented deferrals (`docs/deferred.md`): the
inventory-mapping governance re-review, and `#[ignore]`d async-under-debugger tests
(ASYNC-DEBUG-1). Deferred (needs plan-gap primitives, all in deferred.md): F2-RESIDUAL
(stateful/streaming I/O still on the runtime-driven AwaitIo transport — async overlap works),
the executor async-tier reactor, the `async/run` transitive barrier, DAP/wasm cooperative-debug
mode, and ASYNC-TIMEOUT-CANCEL-1.


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

## Committed gates (continued)

- **D2** `feat: migrate promises to canonical PromiseRegistry` — AsyncPromise→{id}, all 12
  promise ops structural, legacy promise bridge deleted (−622 lines), LegacyPromise split.
  Verified by 3 independent adversarial passes (combinators/cancellation, await/GC, async/run/
  stress); the supplied-promise-not-cancelled contract holds; 5000-item churn stable; all
  stress examples pass. Two findings fixed/documented (below).
- **fix**: structured-condition numeric fields (`:duration-ms`/`:root-id`/…) emitted as integers
  (were strings, violating the plan contract). ASYNC-RUN-BARRIER-1 documented: `async/run` is a
  ready-drain not the plan's transitive settle-barrier; a naive barrier reintroduces
  self-await/channel-rendezvous deadlocks, so the safe drain stays (plan-owner decision).
- **D3** `feat: migrate channels to canonical ChannelRegistry` — Channel→{id}, 9 channel ops
  structural, channel bridge deleted, LegacyChannel split (scheduler.rs needed no edits). Full
  channel now BLOCKS when full (plan-conformant). Bonus: `(map channel/recv ...)` now works
  (yielding natives as direct HOF callbacks). Green + stress examples pass.
- **E** `feat(stdlib): HOFs emit cooperative Call via structural ABI` — map/filter/foldl/reduce/
  for-each/sort-by dual-ABI runtime_func returning Call directly (continuation state machines
  unchanged). NativeYield kept (I/O still used it).
- **F1** `feat(stdlib): external-I/O ops return Suspend structurally; delete NativeYield bridge`
  — the 12 io/system/llm/mcp ops already on WaitKind::External moved off the NativeYield bridge
  to structural returns; **NativeYield + PENDING_NATIVE_OUTCOME fully DELETED** (grep empty).
- **F2 reference** `feat(stdlib): convert http I/O to structural WaitKind::External` — http ops
  off the AwaitIo(IoHandle) bridge to runtime_offload::external_io_interruptible. **Foundation
  finding:** the executor async tier is reactor-less (sema-vm carries no tokio runtime), so
  `interruptible_async` panics on a real reqwest future; offload uses `interruptible_blocking` +
  `io_block_on` + a `tokio::select!` cancel race (preserves abort-on-cancel). Latent same bug in
  sema-llm's interruptible_async path (never run with real network — only FakeProvider).

## Known pre-existing red (fix in Step I)
`llm_fake_test::agent_turn_boundary_collects_between_tool_turns` fails on a GC `:pruned >= 900`
heuristic — CONFIRMED pre-existing (fails identically at the parent commit before F1). Most
likely the threshold shifted when D2/D3 removed the promise/channel GC candidates (fewer objects
to prune). Update the threshold to the new GC behaviour in Step I.

## Remaining sequence

- **F2 fan-out (in progress)**: direct-fit modules (proc/git/sqlite/kv/serial/system) →
  runtime_offload helper (interruptible_blocking for sync git2/rusqlite; io_block_on for async).
- **F2 streaming**: ws/pty/serial/stream operate on a shared VM-thread-held connection; each
  recv/send/connect is one-shot at the AwaitIo level but the connection must be worker-accessible
  — assess tractability vs. documented deferral.
- **F2 sema-llm/mcp**: convert their AwaitIo sites + fix the latent interruptible_async bug (or
  route through the blocking helper).
- **F2 finalize**: delete AwaitIo/IoHandle/io_waits/poll_io_waits/io_park/notify_io_complete +
  the legacy_io_wakeup arm in run_exprs_via_runtime — only once ALL I/O is converted.
- **Historical (superseded)**: the original D2 line below is kept for the record.

- **D2** (done — see above): LegacyPromise split + AsyncPromise{id} reshape + convert 12 promise
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
