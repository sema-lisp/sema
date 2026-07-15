# Task 03: Interpreter-Owned Scheduler and VM Continuations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the thread-local scheduler with one interpreter-owned,
multi-root cooperative runtime whose only evaluator is the bytecode VM.

**Architecture:** `sema-vm::runtime::Runtime` owns root/task tables, ready queues,
timers, wait registrations, external completions, settlement sequencing, and
cleanup. Each task owns a suspended VM plus an explicit continuation stack and
task context. A drive turn drains bounded completions/timers, visits roots in
round-robin order, runs bounded VM quanta, and returns control to the host.

**Tech Stack:** Rust, `sema-core` runtime contracts, `sema-vm`, `sema-eval`,
deterministic clocks, Cargo integration tests.

## Execution contract

- **Status:** Ready only after Task 02 is accepted and committed.
- **Dependencies:** Checked core types, native continuation protocol, explicit
  task context, send-only completion envelope, and Task 02 review.
- **Immutable inputs:** Master runtime ownership, roots/shared globals,
  lifecycle, fair scheduling, drive turns, waits, shutdown, memory, and debugger
  contracts.
- **Exact start state:** Clean worktree; the accepted Task 02 implementation
  commit is an ancestor of `HEAD`; later chronology commits are allowed. Task
  01–02 gates are GREEN except the exact Task 03/04 RED cases recorded in Task
  02 evidence.
- **Parallel work:** State-machine/queue tests and fake-clock/wait tests may run
  in parallel. VM continuation/upvalue edits have one owner; interpreter/runtime
  composition starts after the internal runtime tests merge; debugger work
  starts after root driving exists. Review is independent and post-integration.

## Global constraints

- Tasks 01 and 02 must be accepted and committed before this layer starts.
- The runtime is a field of `Interpreter`; no process-global or thread-local
  scheduler may own runnable tasks.
- The VM remains the sole evaluator. A native continuation resumes through VM
  frames; it may not call a tree-walker or recursively run a second scheduler.
- Multiple roots are mandatory. They share globals but retain independent
  result, cancellation, output, and initial task context.
- FIFO order holds within a root. Runnable roots are visited round-robin.
- Every turn is bounded by work items and bytecode instructions. Wall-clock
  time alone is not a deterministic budget.
- Debugger stop is interpreter-wide. Ordinary root failure/cancellation is not.
- Keep legacy language surface behavior through explicit adapters until Task 04;
  do not preserve the legacy scheduler itself.
- No profiling or benchmarking in this layer.

---

## Files and responsibilities

**Create**

- `crates/sema-vm/src/runtime/mod.rs` — public runtime API and invariants.
- `crates/sema-vm/src/runtime/root.rs` — root records and handles.
- `crates/sema-vm/src/runtime/task.rs` — task state machine and relations.
- `crates/sema-vm/src/runtime/ready.rs` — per-root FIFO and root rotation.
- `crates/sema-vm/src/runtime/timer.rs` — monotonic timer heap and test clock.
- `crates/sema-vm/src/runtime/wait.rs` — wait registration/generation checks.
- `crates/sema-vm/src/runtime/promise.rs` — minimal final four-state promise
  registry and observation sets required by the Task 03 language ABI bridge.
- `crates/sema-vm/src/runtime/channel.rs` — minimal final channel identity and
  wait registry; Task 04 completes the public channel contract.
- `crates/sema-vm/src/runtime/drive.rs` — bounded turn orchestration.
- `crates/sema-vm/src/runtime/cleanup.rs` — cancellation and quarantine reaping.
- `crates/sema-vm/src/runtime/debug.rs` — stop-the-world debugger state.
- `crates/sema-vm/src/runtime/tests.rs` — deterministic internal state tests.
- `crates/sema/tests/runtime_roots_test.rs` — public multi-root/fairness tests.
- `docs/plans/evidence/unified-cooperative-runtime/task-03.md` — command and
  state-transition evidence.
- `docs/plans/reviews/unified-cooperative-runtime/task-03.md` — independent
  review report.

**Modify**

- `crates/sema-vm/src/lib.rs` — export runtime API.
- `crates/sema-core/src/runtime/native.rs` — add traced runtime requests and
  promise-set waits; `NativeCallContext` remains capability-free.
- `crates/sema-core/src/runtime/executor.rs` — expose one doc-hidden universal
  wait-identity issue path used by internal and external waits.
- `crates/sema-core/src/value.rs` and `crates/sema-core/src/cycle.rs` — add
  checked promise/channel handles backed only by runtime identity.
- `crates/sema-vm/src/vm.rs` — resumable native frames, instruction quanta,
  shared captured-cell reads/writes, safe points, and suspend results.
- `crates/sema-vm/src/debug.rs` — replace ambiguous yield variants with explicit
  quantum/native/debug results.
- `crates/sema-vm/src/scheduler.rs` — reduce to a temporary source-compatible
  adapter over `Runtime`; delete task storage and TLS ownership.
- `crates/sema-core/src/async_signal.rs` — remove scheduler ownership callbacks;
  retain only named adapters needed by unmigrated Task 04 builtins.
- `crates/sema-eval/src/eval.rs` — add the runtime field, prepare root VMs, and
  route synchronous eval through submit-and-drive.
- `crates/sema-eval/src/lib.rs` — export root/drive types needed by hosts later.
- `crates/sema-eval/src/debug_session.rs` — bind debug state to the interpreter
  runtime instead of a parallel execution loop.
- `crates/sema-stdlib/src/async_ops.rs` — move existing suspending language
  entry points onto the native continuation ABI before removing the scheduler
  ABI; final promise/channel/ownership semantics remain Task 04.
- `crates/sema-stdlib/src/system.rs` — move the compatibility sleep producer to
  the runtime timer path; only inventoried `AwaitIo` producers retain signals.
- `crates/sema-stdlib/src/io.rs` — make legacy I/O pollers return data only;
  streaming callbacks execute as separately charged same-task native calls.
- `crates/sema/src/lib.rs` and `crates/sema-wasm/src/lib.rs` — replace direct
  `Interpreter` struct literals with the runtime-aware constructor.
- `crates/sema/src/main.rs`, `crates/sema/src/workflow_view/ingest.rs`,
  `crates/sema-dap/src/server.rs`, and `crates/sema-mcp/src/tools.rs` — classify
  direct VM host execution and route it through root preparation or an explicit
  Task 07-owned host boundary.
- `crates/sema/tests/vm_async_test.rs` — turn scheduler/captured-cell
  characterization from RED to GREEN without changing its oracle.
- `docs/internals/async-runtime-inventory.md` and the legacy baseline — record
  removed and remaining adapters.

## Exact runtime interfaces

```rust
pub struct Runtime {
    state: Rc<RefCell<RuntimeState>>,
}

pub struct VmRootOptions {
    pub context: TaskContextHandle,
    pub output: Rc<dyn OutputSink>,
}

pub struct PreparedRoot {
    vm: VM,
}

pub struct RootHandle {
    runtime: Weak<RefCell<RuntimeState>>,
    id: RootId,
}

pub struct DriveBudget {
    pub work_item_limit: NonZeroUsize,
    pub completion_limit: NonZeroUsize,
    pub timer_limit: NonZeroUsize,
    pub root_visit_limit: NonZeroUsize,
    pub cleanup_limit: NonZeroUsize,
    pub instruction_limit_per_task: NonZeroUsize,
    pub wall_clock_limit: Duration,
}

pub enum DriveState {
    Progress {
        work_items: usize,
        instructions: usize,
        ready_remaining: bool,
    },
    Idle {
        next_deadline: Option<Instant>,
        inbox_wakeup_required: bool,
        legacy_io_wakeup_required: bool,
    },
    Quiescent,
    DebugStopped(DebugStop),
    ShutdownComplete(ShutdownReport),
}

pub struct ShutdownOptions {
    pub deadline: Instant,
    pub drive_budget: DriveBudget,
}

pub enum RuntimeCreateError {
    IdExhausted,
    ExecutorAttach(ExecutorAttachError),
}

pub enum SubmitRootError {
    IdExhausted,
    ShuttingDown,
}

pub enum RuntimeFault {
    IdExhausted { kind: &'static str },
    Invariant { message: String },
}

pub enum RootPoll {
    Pending,
    Ready(Rc<TaskSettlement>),
    RuntimeDropped,
}

impl Runtime {
    pub fn new(
        context: Rc<EvalContext>,
        clock: Rc<dyn RuntimeClock>,
        executor: Arc<dyn IoExecutor>,
    ) -> Result<Self, RuntimeCreateError>;
    pub fn submit_root(
        &self,
        prepared: PreparedRoot,
        options: VmRootOptions,
    ) -> Result<RootHandle, SubmitRootError>;
    pub fn drive(&self, budget: &DriveBudget) -> Result<DriveState, RuntimeFault>;
    pub fn cancel_root(&self, root: RootId, reason: CancelReason) -> bool;
    pub fn shutdown(&self, options: &ShutdownOptions) -> Result<ShutdownReport, RuntimeFault>;
    pub fn close_for_interpreter_drop(&self);
}

impl RootHandle {
    pub fn id(&self) -> RootId;
    pub fn poll_result(&self) -> RootPoll;
    pub fn cancel(&self, reason: CancelReason) -> bool;
}
```

`RuntimeState` is private. `Runtime` is the interpreter-owned strong handle and
is not `Clone`; root handles retain only `Weak` access to the same state.
`RootHandle::clone` upgrades the weak pointer and increments an explicit
runtime-side handle count when the runtime is alive; every drop upgrades and
decrements it when possible. There is no independently cloneable lease.
Settlement remains pollable while that count is nonzero. Final-handle drop is
queued as cleanup, and the root is reaped only after settlement and after all
descendant, debugger, output, and tracing retention reaches zero.

`Runtime` owns the interpreter's `Rc<EvalContext>` because every VM quantum and
the Task 02 legacy-native fallback require it. The runtime installs the active
task's `TaskContextHandle` only for the duration of legacy callback invocation;
runtime-aware natives receive only `NativeCallContext`. `PreparedRoot` is opaque
and contains a VM whose initial frame was already installed by a fallible
`VM::prepare_entry(Rc<Closure>)`; it does not carry a second `Value` that can
disagree with the VM entry.

`Runtime::new` creates its thread-safe completion inbox, calls
`CompletionRegistrar::register` to receive a fresh `RuntimeId` plus the
capability-safe registrar, stores the registrar privately, and calls
`executor.attach_runtime(runtime_id)` once. Registrar exhaustion returns
`RuntimeCreateError::IdExhausted`; attachment failure returns
`RuntimeCreateError::ExecutorAttach(error)` without misclassifying it as ID
exhaustion. Duplicate IDs and attachment after executor shutdown are attachment
errors. Runtime state owns the returned
`Arc<dyn ExecutorLease>` and inbox receiver. Tests inject a Task 02 fake
executor/lease; Task 03 has no dependency on `sema-io`. Task 05 wires the
production `sema-io` implementation at host construction sites. Constructor
tests force registrar exhaustion and each executor attachment failure and assert
the exact `RuntimeCreateError` variant.

Task 03 adds fallible `Interpreter::try_new*` and one parts constructor that
accepts an executor. Existing infallible constructors use a documented local
executor that attaches successfully and rejects unsupported external submissions
through the ordinary registered rejection path; they never unwrap attachment or
identity allocation. If runtime identity allocation fails, the compatibility
constructor records a terminal initialization error and every evaluation returns
that error without admitting a root. Direct struct literals in `sema` and
`sema-wasm` are replaced by the parts constructor. Task 05 replaces the local
executor at native host construction sites, and Task 07 finalizes host-facing
fallible construction.

`RootPoll::Ready` returns the retained `Rc<TaskSettlement>`, preserving the
three-way returned/failed/cancelled outcome and its `SettlementSeq`. Polling
never drives, takes, or mutates runtime state. It is idempotent for every live
handle, including after the main task record is reaped. `RuntimeDropped` means
the weak runtime pointer no longer upgrades, not that a result was consumed.

```rust
pub enum TaskState {
    Ready,
    Running,
    Waiting(WaitKey),
    Settled(Rc<TaskSettlement>),
}

pub enum VmExecResult {
    Finished(Value),
    Failed(SemaError),
    Native(NativeOutcome),
    QuantumExpired,
    DebugStopped(DebugStop),
}

pub enum RuntimeRequest {
    Spawn {
        callable: Value,
        continuation: Box<dyn NativeContinuation>,
    },
    CancelPromise {
        promise: PromiseId,
        continuation: Box<dyn NativeContinuation>,
    },
    CreateChannel {
        capacity: usize,
        continuation: Box<dyn NativeContinuation>,
    },
    ChannelOp {
        channel: ChannelId,
        operation: ChannelOperation,
        continuation: Box<dyn NativeContinuation>,
    },
    CreateSettledPromise {
        outcome: TaskOutcome,
        continuation: Box<dyn NativeContinuation>,
    },
    InspectPromise {
        promise: PromiseId,
        continuation: Box<dyn NativeContinuation>,
    },
    OriginBarrier {
        continuation: Box<dyn NativeContinuation>,
    },
}

pub enum ChannelOperation {
    Close,
    TryReceive,
    Inspect(ChannelQuery),
}

pub enum ChannelQuery { Closed, Count, Empty, Full }

pub enum PromiseSetMode {
    All,
    Race,
    Timeout(Duration),
}

pub struct PromiseSetWait {
    pub promises: Vec<PromiseId>,
    pub mode: PromiseSetMode,
}

pub struct CancellationRequest {
    pub reason: CancelReason,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WaitKey {
    pub id: WaitId,
    pub generation: WaitGeneration,
}

pub enum RootState {
    Running { main_task: TaskId },
    Settled(Rc<TaskSettlement>),
}

pub struct RootRecord {
    id: RootId,
    state: RootState,
}

pub struct TaskRecord {
    id: TaskId,
    relations: TaskRelations,
    state: TaskState,
    cancellation: Option<CancellationRequest>,
}

pub enum StateName { Ready, Running, Waiting, Settled }

pub enum TaskTransitionError {
    Invalid { from: StateName, to: StateName },
    WaitMismatch { expected: WaitKey, actual: WaitKey },
}

pub enum RootTransitionError {
    WrongMainTask { expected: TaskId, actual: TaskId },
    AlreadySettled,
}

/// Exists only after `Runtime::apply_native_suspend` registers an external
/// request; it is never a second unregistered request type.
struct RegisteredExternalWait {
    identity: WaitIdentity,
    task_id: TaskId,
    decoder: Box<dyn CompletionDecoder>,
    resource: ResourceClass,
    queue_cancel: ExecutorCancelHandle,
    continuation: Box<dyn NativeContinuation>,
}
```

`TaskRecord::new(id, relations)` starts in `Ready`. The exact legal edges are
`start: Ready -> Running`, `yield_ready: Running -> Ready`,
`wait: Running -> Waiting(key)`, `wake: Waiting(same key) -> Ready`, and
`settle: Ready|Running|Waiting -> Settled`. `wake` with a different key returns
`TaskTransitionError::WaitMismatch`; every other illegal edge returns
`TaskTransitionError::Invalid { from, to }`. `RootRecord::new(id, main_task)`
starts in `Running { main_task }`; `RootRecord::settle(main_task, settlement)`
accepts only that same task and retains the exact `Rc<TaskSettlement>`, while a
wrong task or duplicate settlement returns `RootTransitionError`.

All transitions return `Result` in every build;
debug/test builds additionally assert store/queue agreement after successful
transitions. `Running -> Running`, duplicate settlement, and every transition
out of `Settled` return an error naming stable `StateName` values. `wait`
accepts one universal `WaitKey`. `settle(sequence, outcome)` constructs, stores,
and returns the sole `Rc<TaskSettlement>`; the record never owns the sequence
allocator. `request_cancellation(reason) -> bool` is idempotent and first-reason
wins. Cleanup progress is side state on the task/cleanup registry rather than a
lifecycle variant, and reaping removes a settled record from `TaskStore`;
neither changes the master lifecycle.

`ReadyRoots` stores one FIFO `VecDeque<TaskId>` per `RootId` plus a rotating
`VecDeque<RootId>`. Enqueuing a second ready task for an already-present root
does not enqueue the root twice. After one task quantum, a still-runnable root
moves to the back. Within that root, a yielded task moves behind tasks already
ready there.

Wait lookup uses `(WaitId, WaitGeneration)`. `WaitIdentity` stores the complete
`RuntimeId`, `WaitId`, `WaitGeneration`, `OperationId`, and `CompletionKind`.
Each active registration is the sole owner of that identity, decoder,
`ResourceClass`, queue-control half, continuation, and waiting task relation.
`CompletionRegistrar` owns the runtime's only `WaitId` and `WaitGeneration`
counters. A doc-hidden `issue_wait_identity()` returns the runtime-scoped pair
for timers, promises, channels, barriers, and legacy-I/O waits.
`issue_identity(kind)` calls that same allocator and adds the external-only
`OperationId`, kind, and binding authority. No second runtime/VM wait counter
exists, so internal and external wait keys cannot collide.
`RegisteredExternalWait` is the only registered external shape. Its `Trace`
implementation delegates to its decoder and continuation with exact direct
multiplicity. The record remains an external root despite implementing `Trace`;
it is not a collector candidate unless separately installed as an opaque
payload through the existing payload-tracer registration. Cleanup/resource
hooks follow the Task 02 trace contract and must not capture untraced
`Value`/`Env` state.
Cancellation or completion removes the wait registration before invoking
callbacks. A late or duplicate completion is counted and discarded; it never
resumes another wait.

For a valid completion, removal yields one decoder and one continuation. The
runtime consumes the decoder with the raw send-safe result, maps its sole
`DecodedCompletion` to `ResumeInput::Returned`/`Failed`, then consumes the
continuation and applies its `NativeResult`. Worker failure and decode failure
use that same sequence. Explicit task cancellation removes the registration,
drops the decoder exactly once without invoking it, and consumes the continuation
with `ResumeInput::Cancelled(reason)`. No path can orphan, duplicate, or bypass
either object.

Completion routing checks the full
`(RuntimeId, WaitId, WaitGeneration, OperationId, CompletionKind)` identity in
this order: active wait, then quarantine-cleanup entry. Cancelling a running
`QuarantinedBounded` wait removes/drops its decoder and continuation as above,
but atomically transfers job/resource accounting plus the exact identity and
bound to `CleanupRegistry`. A later exact completion is cleanup-only: discard
its payload, remove/decrement that entry once, and increment
`quarantine_reaped`, without changing a task outcome or `late_completions`.
Wrong identity/kind leaves the cleanup entry live and counts late/fault; a
duplicate after reap is late. Interruptible stale completion remains late even
when failed-hook cleanup is retained, because only `reap` may release that
resource. Completion-versus-cancellation uses the active-wait removal as its
linearization point: completion-first runs decoder/continuation; cancel-first
transfers quarantine ownership before the task may settle. Bound expiry names
an invariant failure and keeps ownership until completion/reap makes accounting
safe.

`Runtime::apply_native_suspend` owns one external-registration transaction. It
reads `NativeSuspend.wait`; for `WaitKind::External`, it reads
`prepared.completion_kind()` to issue the complete identity before moving the
sole `Box<PreparedExternalOperation>` into registrar binding. Its private
registrar is the sole allocator: `CompletionRegistrar::issue_identity(kind)`
allocates `OperationId`, `WaitId`, and `WaitGeneration` and returns the complete
private-capability identity. The registrar then binds that runtime-issued
identity and prepared operation and
splits the binding: the runtime receives the decoder, sole active
`ResourceClass`, and queue-control half, while the executor-facing half is one
opaque `ExecutorSubmission`. A private pending-registration owner holds all
parts until one atomic state transition installs `RegisteredExternalWait`,
increments live-resource accounting, and changes the task from `Running` to
`Waiting`. Active resources are not cleanup entries. Only after that transition
commits and the `RuntimeState` borrow is released does the runtime submit the
opaque `ExecutorSubmission`. `sema-io` cannot name the
sink or obtain a registrar for this runtime. The executor cannot drain the
completion inbox reentrantly during this transition, so
an executor that completes inline during `submit` sees fully registered waiting
state; its completion is processed only after `apply_native_suspend` returns.
There is no second `ExternalWait`, double registration, or clone/move of the
decoder, resource, queue handle, continuation, job, or start token.
No producer or executor allocates a runtime identity.

Normal completion removes the active wait and its resource. Cancellation or
rejection invokes the active resource's one-shot policy; only `PendingReap`, a
hook error, or admitted quarantined work transfers that sole resource owner to
`CleanupRegistry`. No active operation is simultaneously represented in the
cleanup registry.

Every callback transition follows extract/invoke/apply: under one short
`RuntimeState` borrow, remove the registration and extract decoder,
continuation, VM/task bookkeeping, and pending transition; drop that borrow;
construct `NativeCallContext` from mutable task context and a cancellation
snapshot and invoke decoder/callback/continuation; then reborrow state only to
validate and apply the returned transition. VM execution of
`NativeOutcome::Call` dispatches every callable form through the same task VM.
Native callables use doc-hidden `NativeFn::invoke_runtime`; VM closures,
keywords, multimethods, and other callable forms use the VM's ordinary call
dispatch. This remains explicit runtime work, not a capability on the context.
Tests cover suspend/resume/suspend
through one continuation, a callback that enqueues a completion while running,
and nested native-to-Sema-to-native suspension; none may panic from a nested
`RefCell` borrow or process the enqueued completion reentrantly.

Submission rejection returns an opaque owning error. Its consuming `rollback`
destroys the unarmed sink, job, and start token inside `sema-core` and returns
only the rejection kind. The same transaction removes the wait, takes the
wait-owned resource entry, performs its one-shot cancellation, drops both
queue-control halves, transitions `Waiting -> Running`, and consumes the
registered decoder with `Err(ExternalFailure { code: Rejected, ... })`. It maps
that `DecodedCompletion` to `ResumeInput::Returned`/`Failed` and then consumes
the already-registered continuation.
`Reaped` removes the resource; `PendingReap`/error atomically transfers it to
retained cleanup before resumption. The returned `NativeResult` is applied
through the ordinary native-result path. Rollback cannot enqueue a completion
or leave a parked task.

If an interruptible cancel hook returns `CancelHookError`, cancellation remains
sticky and the task's primary cancelled outcome is unchanged. The runtime moves
the hook/resource entry to cleanup retry state with its operation/resource
identity, reap-attempt count, and deduplicated suppressed diagnostic.
Live-resource accounting is unchanged. Repeated task cancellation and the
registry never call `cancel` again; bounded cleanup turns invoke only the hook's
nonblocking `reap` method. A successful `Reaped` removes/decrements the entry
exactly once. Task cancellation may settle only after the transfer into cleanup
ownership is recorded. At shutdown deadline, any retained entry makes the
report non-clean and emits a named invariant failure with its last error and
attempts.

The runtime assigns `SettlementSeq` at the single transition into `Settled`.
Sequence assignment occurs before waking observers, so pre-settled promise races
can be ordered later without list-order bias.

Every checked allocator failure is handled without wrapping, sentinels,
`unwrap`, or `expect`. Root/task allocation fails submission. Wait/operation and
generation exhaustion fails the running operation only after restoring every
extracted owner to a valid task/wait/cleanup record. Settlement-sequence
exhaustion is a terminal `RuntimeFault`: the runtime rejects new submissions,
preserves existing sequenced settlements for polling, and begins bounded
cancellation and cleanup rather than publishing an unsequenced outcome.

One drive turn has a global work-item budget in addition to its source-specific
caps. Dequeueing one completion, firing one timer, attempting one cleanup reap,
visiting one ready task, invoking one native callable, resuming one native
continuation, or applying one resulting native transition consumes one work
item. A native transition ends the current root visit; no immediate call/resume
loop can run without consuming another global credit. VM dispatch charges one
instruction per decoded opcode and checks before the next opcode after saving
the current PC. Credits do not reset across VM/native/VM transitions. Bounded
source rounds reserve eligible root visits so a completion or timer storm cannot
consume the entire turn before ready work runs. The wall-clock limit is only a
secondary host-latency guard checked between work items.

Each native continuation frame records the caller frame depth, original call
site PC, tail/non-tail disposition, and sole continuation. A Sema callee runs on
the same task VM. Return or an uncaught error at that boundary stops VM dispatch
and becomes `ResumeInput::Returned` or `ResumeInput::Failed`; it does not resume
the bytecode caller first. Quantum expiry preserves the complete bytecode and
native-boundary stacks. A continuation returning another `Call` or `Suspend`
replaces or extends the boundary without cloning it. Final native return pushes
one result according to the saved call disposition; final native failure enters
the ordinary exception machinery at the saved native call site. Child-task VM
creation is the only operation that detaches open upvalues, and it shares the
existing `UpvalueCell` rather than copying captured state. Task VMs and stacks
remain explicit external GC roots; the runtime does not register an entire VM or
task record as an opaque payload.

Task 03 extends `NativeOutcome` with `Runtime(RuntimeRequest)` and `WaitKind`
with `PromiseSet(PromiseSetWait)`. These are traced, consuming commands handled
by `Runtime` after VM dispatch returns; they do not put a runtime handle or
command capability on `NativeCallContext`. `Spawn` creates the runtime task and
its stable promise handle, `CancelPromise` targets that checked identity,
`CreateChannel` allocates the final checked channel identity, `OriginBarrier`
registers a generation-aware wait, `CreateSettledPromise` allocates a sequenced
returned/failed/cancelled synthetic promise, `InspectPromise` returns the exact
pending/returned/failed/cancelled state used by predicates, and promise-set
waits implement the existing all/race/timeout mechanism. Each command/settlement resumes its sole
continuation and consumes a work item. The promise registry uses the final
pending/returned/failed/cancelled partition and canonical
`Rc<TaskSettlement>`; the channel registry owns buffered values and
generation-checked waiters. Task 04 completes public semantics, predicates,
structured ownership, and validation on these same records rather than
replacing a temporary state model.

## Task 1: Implement root/task state machines from tests

**Files:** `runtime/mod.rs`, `runtime/root.rs`, `runtime/task.rs`,
`runtime/tests.rs`, `crates/sema-vm/src/lib.rs`

- [x] **Step 1: Write failing transition-table tests**

Test every legal edge and representative illegal edges. Include returned,
failed, and cancelled settlements and prove one canonical
`Rc<TaskSettlement>` is assigned once. Test that origin root, cancellation
parent, and lifetime owner never change when a task changes state. This first
slice tests pure records only; root-handle retention, submission exhaustion,
terminal runtime faults, and reaping are tested in Task 3 after runtime state,
drive, and cleanup exist.

- [x] **Step 2: Implement the records and transition methods**

Use checked IDs from Task 02. Do not expose mutable task/root fields outside the
runtime module. Task 1 owns initial module scaffolding and the private test
module so later queue/wait workers add submodules without competing for setup.
The pure root record contains identity and state only; Task 3 adds checked
retention counters with handle/reaping tests, and Task 6 adds initial context and
output ownership when root submission is composed.

- [x] **Step 3: Run**

```bash
cargo test -p sema-vm runtime::tests::state
```

Expected: transition table passes; invalid edges fail with the named old/new
states.

## Task 2: Implement deterministic fair ready queues

**Files:** `runtime/ready.rs`, `runtime/tests.rs`

- [x] **Step 1: Write failing sequence tests**

Assert exact dequeue sequences for:

- roots A/B/C with one perpetually requeued task each: `A B C A B C`;
- root A with tasks A1/A2/A3 and root B with B1: `A1 B1 A2 B1 A3 B1`;
- removal of a settled root without disturbing remaining order;
- duplicate wake attempts for one task.

- [x] **Step 2: Implement queue invariants and duplicate protection**

Keep membership sets private and assert queue/set agreement after mutations in
test builds.

- [x] **Step 3: Run**

```bash
cargo test -p sema-vm runtime::tests::ready
```

Expected: exact order assertions pass.

## Task 3: Implement timers, completions, and stale-delivery rejection

**Files:** `runtime/timer.rs`, `runtime/wait.rs`, `runtime/drive.rs`,
`runtime/tests.rs`

- [x] **Step 1: Add a fake monotonic clock and failing tests**

Cover same-deadline insertion order, zero duration, timer cancellation,
generation reuse, wrong runtime, wrong operation, wrong completion kind,
correct-kind payload decode failure, duplicate completion,
completion-vs-cancellation races, per-turn completion/timer/cleanup/root limits,
and wall-clock budget expiry measured with an injected clock. Cover
`Runtime::new` registrar exhaustion as `RuntimeCreateError::IdExhausted` and
duplicate-runtime/shutdown attachment failures as
`RuntimeCreateError::ExecutorAttach` carrying the exact `ExecutorAttachError`.
Wrong-kind
delivery must leave the wait/task outcome unchanged and must not invoke the
decoder; correct-kind decode failure is delivered as the Task 02 `Decode`
failure. Add fake hooks whose one-shot `cancel` returns `PendingReap` and `Err`:
cleanup retains ownership and live-resource count, repeated task cancellation
does not invoke `cancel` again, later cleanup turns invoke only `reap`, and
`Reaped` decrements once. A persistent reap error remains named in snapshots and
suppressed diagnostics, and makes shutdown non-clean at its deadline.
Use counted consuming decoder/continuation fakes to prove exactly one fate for
each on success, worker failure, decode error, explicit cancellation, and submit
rejection. Cancellation drops (does not invoke) the decoder once and invokes the
continuation once with `Cancelled`; rejection traverses decoder then
continuation. The inline-completion and rejection tests must exercise the exact
order: inspect completion kind and issue identity; bind and split; atomically
install the registered wait, resource, and `Waiting` state; then submit.
Add quarantine routing tests for both completion-vs-cancel orders, exact
cleanup-only completion, wrong-kind cleanup completion, duplicate after reap,
bound expiry, and shutdown returning zero live cleanup after an in-bound
completion. A cancelled observer/task outcome never changes when cleanup-only
completion arrives.
Add completion and timer backlogs larger than both their source caps and the
global work-item cap. Assert ready roots still receive reserved visits, reported
work never exceeds `work_item_limit`, and no task receives two VM quanta in one
root visit.
Poll returned, failed, and cancelled root settlements repeatedly through two
cloned handles; drop either clone first; reap the main task while retaining the
other handle; and prove only final-handle drop makes the root reap-eligible.
Force root, task, wait/operation, and settlement-sequence exhaustion and assert
the exact submission/operation or terminal runtime-fault path without an
unsequenced settlement or leaked extracted owner.

- [x] **Step 2: Implement registration and bounded drains**

Do not sleep in `Runtime::drive`. Return `Idle { next_deadline }`; native hosts
may wait outside the runtime and browser hosts may schedule a macrotask.

- [x] **Step 3: Run**

```bash
cargo test -p sema-vm runtime::tests::timer
cargo test -p sema-vm runtime::tests::wait
cargo test -p sema-vm runtime::tests::drive_limits
```

Expected: deterministic tests pass without wall-clock delays.

## Task 4: Make the VM resumable and migrate the suspending ABI

**Files:** `vm.rs`, `debug.rs`, `runtime/task.rs`, `runtime/tests.rs`,
`async_signal.rs`, `scheduler.rs`, `async_ops.rs`, `system.rs`, `io.rs`

- [x] **Step 1: Write failing VM continuation tests**

Test native return, native call into a Sema closure, suspend/resume with each
outcome, nested native-to-Sema-to-native calls, exception propagation across a
continuation, cancellation at a safe point, and repeated quantum expiry.
Add an immediate native call/continuation chain longer than `work_item_limit`
and a zero-duration suspend chain; each drive turn must stop at the exact cap.

- [x] **Step 2: Add explicit continuation frames**

Store them in traceable VM/task state. Remove the `set_yield_signal` plus dummy
`nil` protocol from the VM dispatch path only after Step 4 migrates every
reachable suspending builtin. A native call returns
`VmExecResult::Native`; the runtime executes or registers it, then resumes the
same VM through an explicit frame.

- [x] **Step 3: Add instruction budgeting**

Check the budget at dispatch safe points and return `QuantumExpired` without
losing stack, open-upvalue, handler, or debug state. Zero-work spinning inside a
native continuation is forbidden: each continuation transition consumes a
drive work item.

- [ ] **Step 4: Move the existing language suspension mechanism onto the runtime**

Add a temporary `LegacyAsyncAbiAdapter` used only by Task 04-owned language
semantics. It translates existing `async/spawn`, pending `async/await`, sleep,
blocked channel operations, `async/cancel`, `async/all`, `async/race`,
`async/timeout`, `async/run`, synthetic promise constructors, and promise
predicates into `NativeOutcome::{Return, Call, Suspend, Runtime}` over the final
Task 03 runtime request and identity registries.
It owns no task table, ready queue, timer heap, clock, scheduler target loop, or
strong runtime handle; performs no TLS runtime lookup; and never calls
`Runtime::drive`, `Interpreter::eval*`, `VM::execute`, or
`call_run_scheduler*`. Each adapter continuation owns its observation/wait state
and is consumed exactly once. It may preserve only the existing observable
language behavior assigned for correction in Task 04; it may not recreate a
second scheduler, promise ownership model, or cancellation tree. Record every
adapted builtin in the inventory with Task 04 as deletion owner.

The adapter is deleted in Task 04 after runtime-native promise observation,
detached spawn, combinators, and channels exist, before Task 04 acceptance.
`LegacyRuntimeBridge` may remain only for producers assigned to Tasks 05–08; it
cannot schedule tasks or drive the runtime.

Task 03 separately retains `LegacyAwaitIoBridge` for the exact unmigrated
producer call sites assigned to Tasks 05–08. It is not a scheduler: the VM may
inspect only `YieldReason::AwaitIo` after a legacy native returns, discard that
native's placeholder `nil`, and install the `Rc<IoHandle>` as one runtime-local
polled wait. The bridge obtains one universal registrar-issued `WaitKey`,
installs the keyed record, and commits `Running -> Waiting(key)` before polling;
completion and cancellation remove that exact key before resumption or abort.
Each nonblocking poll consumes one drive work item; pending handles
remain parked and are revisited only in a later bounded source round; completion
resumes the original VM call frame directly; cancellation invokes `abort` once.
The bridge owns no task store, clock, ready queue, recursive drive callback, or
resume-value slot. `set_yield_signal`/`take_yield_signal` and producer-side
`take_resume_value` may remain only for the exact inventoried `AwaitIo` callers;
all promise/channel/sleep/debug uses are deleted in Task 03. Tasks 05–08 replace
those callers with registered external operations and delete the bridge.
When a legacy-I/O wait is pending, `DriveState::Idle` sets
`legacy_io_wakeup_required`. `async_signal` exposes a generation snapshot and
`io_park_since(generation, timeout)` so notification between drive and park is
observed rather than missed. Because the legacy condvar and completion inbox
cannot be selected together, native compatibility hosts cap every legacy-I/O
park at `LEGACY_IO_POLL_MAX = 10ms` and use the minimum of that cap and the next
timer deadline; inbox work is therefore delayed by at most the bridge cap.
Browser hosts schedule a macrotask while any legacy-I/O wait remains.
`Runtime::drive` itself never waits or polls an unbounded batch.

Legacy I/O poll closures return result data only. They may not invoke Sema
callbacks or create a VM. Streaming/file callbacks previously reached through
`run_closure_foreign_sync` are extracted from the poller and invoked through
`NativeOutcome::Call` on the waiting task; each callback and continuation
transition consumes its own work item and may suspend normally. Focused stream
tests prove a callback chain longer than the drive budget is split across turns.

- [ ] **Step 5: Run focused VM suites**

```bash
cargo test -p sema-vm runtime::tests::continuation
cargo test -p sema-vm runtime::tests::quantum
cargo test -p sema-lang --test vm_integration_test
cargo test -p sema-lang --test vm_async_test
```

Expected: all selected tests pass except the exact Task 04 language-contract
RED cases; no nested scheduler entry or dummy-`nil` suspension remains.

## Task 5: Repair shared captured-cell coherence

**Files:** `vm.rs`, `vm_async_test.rs`

- [ ] **Step 1: Preserve the existing failing characterization**

Do not edit its expected Sema value. Add focused VM tests for parent write then
child read, child write then parent read, alternating writes, frame return, and
cycle collection of a shared cell.

- [ ] **Step 2: Use the tracked cell as the authoritative slot**

Once an open local is detached into `Tracked`, both parent local loads/stores
and child upvalue loads/stores consult the shared cell. Closing the defining
frame preserves the latest shared value exactly once.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test vm_async_test -- captured
cargo test -p sema-vm upvalue
cargo test -p sema-core cycle
```

Expected: captured-mutation characterization turns GREEN and GC tests pass.

## Task 6: Compose the interpreter-owned runtime

**Files:** `runtime/mod.rs`, `runtime/drive.rs`, `eval.rs`, `lib.rs`,
`scheduler.rs`, `async_signal.rs`, `crates/sema/src/lib.rs`,
`crates/sema/src/main.rs`, `crates/sema/src/workflow_view/ingest.rs`,
`crates/sema-wasm/src/lib.rs`, `crates/sema-dap/src/server.rs`,
`crates/sema-mcp/src/tools.rs`, `runtime_roots_test.rs`

- [ ] **Step 1: Add failing multiple-root integration tests**

Using one `Interpreter`, submit roots A and B before driving. Assert independent
handles/output/context; shared global definitions; cooperative last-scheduled
global writes; A cancellation does not cancel unrelated B; and detached work
from normally settled A can finish while B is driven. Drop A's result handle
while that detached task remains and prove the root record/output origin stays
alive until its last descendant/debug/tracing reference is released.

- [ ] **Step 2: Add root preparation and sync compatibility**

`Interpreter::eval*` prepares one root, submits it, and repeatedly drives until
that root settles while still servicing other roots. The method does not wait
for detached tasks. Runtime construction happens once in `Interpreter::new*`.
`Interpreter` stores `ctx: Rc<EvalContext>` and `runtime: Option<Runtime>`; the
`Option` exists only so drop can destroy runtime execution edges before context
and environment collection. Add `try_new*` plus a parts constructor, and update
every direct struct literal in `sema` and `sema-wasm`.

Inventory every production VM constructor/preparer and execution surface,
including `VM::new*`, `prepare_entry`, `execute*`, `run*`, `start_cooperative`,
`run_cooperative`, `execute_compile_result`, `eval_value_vm`, `call_value*`,
`run_closure_foreign_sync`, `run_nested_closure_args`, `call_run_scheduler*`, and
`Runtime::drive`. Classify each as: (A) host root preparation followed by outer
submit-and-drive; (B) same-task nested work represented by a VM/native
continuation frame; (C) bootstrap/compile-time synchronous work with a test
proving suspension is rejected without leaving a wait; (D) a foreign
synchronous boundary that rejects suspension and rolls back cleanly; or (E) an
explicit temporary host boundary in CLI `.semac`, DAP, WASM, notebook, or MCP
with an exact Task 07 deletion owner. Arbitrary user code is never C or D. No
active-task path creates a fresh VM, recursively drives the runtime, or
manufactures a synchronous result from a suspending call. Task 03 updates the
named owner files enough to compile and route host-executable user code through
A or E, adds their focused compile/tests to the gate, and records every
occurrence and classification in evidence.

  - _Progress (2026-07-14): VM-root execution seam proven and a synchronous
    `eval` path routed through the runtime:_
    - _`04e090fa` `VM::seed_main_frame` + `Runtime::submit_vm_root` drive a real
      compiled root through `run_quantum` to `Returned` (`(+ 1 2)` → `3`);
      `c0a8057d` promotes `submit_root` to real API with a `TaskPayload::Vm`;
      `ed7c2208` interleaves two real roots independently._
    - _`9854376a` adds `MonotonicClock`/`NullExecutor` host adapters; `3003aacd`
      adds `Interpreter::eval_via_runtime` — GATE GREEN: an interpreter routes a
      synchronous eval through the runtime (`(+ 1 2)` → `3`)._
    - _TEMPORARY BRIDGE landed (2026-07-14): legacy user closures called across
      context boundaries during a quantum now evaluate through the runtime via
      `EvalContext::suspend_runtime_quantum` (context.rs). A cross-context
      `define`d closure re-enters through `call_value` → the `make_closure`
      native wrapper; with shared globals it runs as a nested frame on the live
      runtime VM (`run_nested_closure_args`), otherwise on a fresh foreign VM
      (`call_closure_owned` / `run_closure_foreign_sync` / the wrapper's fresh-VM
      arm). All those sites are the NON-async, synchronous-only legacy-callback
      re-entry path (the async case routes to `run_closure_as_inline_task`
      first), so they never touch the scheduler; each suspends the quantum flag
      for the nested run. GATE GREEN: `eval_via_runtime_shares_interpreter_globals`
      now passes (un-ignored). The genuine fresh-VM entry guard in `VM::run`
      (vm.rs:1601) still rejects non-suspended re-entry._
    - _STILL PENDING (Task 04 native-ABI migration owns deletion of the bridge):
      the proper fix routes native/closure re-entry through
      `NativeOutcome::Call` (state.rs:1351 → `invoke_callable`) instead of a
      nested/fresh VM under a suspended quantum. Deleting `suspend_runtime_quantum`
      and its call sites is the Task 04 deletion owner._
    - _Progress (2026-07-15): SHARED-CONTEXT GAP CLOSED. `Interpreter.ctx` is
      now `Rc<EvalContext>` and `run_exprs_via_runtime` builds the per-call
      `Runtime` with `Rc::clone(&self.ctx)` instead of a fresh
      `EvalContext::new()`. The runtime stores that ctx as `state._context` and
      uses it as the VM's `eval_context` when driving (state.rs ~1269, ~2089),
      so the VM's `call_value`/`eval_value` re-entry now dispatches through the
      interpreter's REGISTERED callbacks and its LIVE module cache / current-file
      / dynamic context — closing the "call callback not registered" class of
      bug. Refactor was mechanical: only 3 direct `Interpreter { .. ctx }` struct
      literals needed wrapping (`Interpreter::new`/`new_with_sandbox` in eval.rs,
      `sema/src/lib.rs` builder, `sema-wasm` `new_with_options`); every other
      `&interp.ctx` / `interp.ctx.method()` site (~20 across sema, sema-wasm,
      sema-dap, sema-lsp, sema-notebook, sema-mcp) works unchanged via `Rc`
      Deref. GATES GREEN (un-ignored, `mod runtime_eval_tests`): multimethod
      dispatch matches oracle (→ 12), `apply`-dispatched user closure matches
      oracle, multimethod persists across two runtime evals (→ 12, 25),
      `make-parameter`/`parameterize` dynamic context persists across evals.
      Full suites unchanged: eval_test 1072/0, integration_test 1055/0,
      vm_async_test 4 pre-existing RED, sema-vm 0 failed, sema-eval 89 passed.
      The flip of `eval`/`eval_str` onto the runtime is now unblocked by this._
    - _Still pending for Step 2 (box stays unchecked): interpreter-owned SINGLE
      shared-context `runtime: Option<Runtime>` constructed once in
      `Interpreter::new*` (with drop ordering that destroys runtime execution
      edges before context/env collection) rather than the current per-call
      Runtime; `try_new*`/parts constructors; a real executor; and the full flip
      of `eval`/`eval_str` (not just the `*_via_runtime` entry points) onto the
      runtime. The per-call Runtime shares the ctx correctly but rebuilds runtime
      state each eval — fine for synchronous evals, but the persistent runtime is
      required before detached cross-eval tasks can survive._
    - _Progress (2026-07-15): PERSISTENT INTERPRETER-OWNED RUNTIME LANDED.
      `Interpreter` now holds `runtime: Option<Runtime>`, constructed ONCE in
      `Interpreter::new`/`new_with_sandbox` via a new `Interpreter::from_parts`
      parts constructor (`build_runtime` shares `Rc::clone(&ctx)` +
      `MonotonicClock` + `NullExecutor`); the three external struct literals
      (`sema/src/lib.rs` builder, `sema-wasm` `new_with_options`) route through
      `from_parts`. `run_exprs_via_runtime` no longer builds a per-call
      `Runtime` — it `submit_root`s a fresh ROOT to the shared runtime and
      drives that root to settlement while detached tasks from prior evals
      advance fairly alongside (`poll_result` settles on the requested root
      only). Drop ordering (eval.rs ~90): a BOUNDED `runtime.shutdown`
      (finite 2s deadline + `DriveBudget::host_default`) cancels+reaps all
      tasks and the runtime is dropped BEFORE the `EvalContext` value-store
      clear + `global_env` release + `InterpreterDrop` collection, so runtime
      task/promise/channel edges cannot pin the env past teardown; while
      unwinding the field is left to its own bounded `close_for_interpreter_drop`
      (no VM driving). GATES GREEN (un-ignored, `mod runtime_eval_tests`):
      `runtime_detached_spawn_survives_across_evals` (spawn+define p in one
      eval, `await p` → 42 in a SECOND eval — cross-eval detached survival) and
      `runtime_drop_with_detached_timer_parked_task_does_not_hang` (drop with a
      100000ms-sleep-parked detached task is bounded <2s). Full suites: sema-eval
      91/0, sema-vm 0 failed, eval_test 1072/0, integration_test 1055/0,
      vm_async_test 4 pre-existing RED, leak_test 7/0, gc_stress_test 48/0,
      clippy+fmt clean. REMAINING Step-2 item: the actual flip of `eval`/`eval_str`
      (not just the `*_via_runtime` entry points) onto the runtime + a real
      executor / async-I/O (`NullExecutor` still rejects real I/O)._
    - _Progress (2026-07-15): EVAL FLIP MEASURED → REVERTED (not yet landable).
      Routed `eval`/`eval_str` (`eval_in_global`/`eval_str_in_global`) through
      `run_exprs_via_runtime`. Two oracles held: eval_test 1072/0,
      integration_test 1055/0 — but ONLY after fixing a re-entry-guard gap: the
      `eval`/`load`/`import` builtins re-enter the VM synchronously via
      `eval_value_vm`/`eval_module_body_vm` → `VM::execute` → `run`, which the
      runtime-quantum guard (`vm.rs:1662 ctx.runtime_quantum_active()`) rejects
      with "legacy native callback cannot re-enter a VM during an active runtime
      quantum" (18 integration_test failures: eval/load/import/module tests).
      Fix mirrored the existing `run_nested_closure_args` bridge — suspend the
      quantum for the duration of `VM::execute` (`ctx.suspend_runtime_quantum()`),
      since `execute` is the legacy synchronous run-to-completion entry the
      runtime never drives through (it uses `seed_main_frame`+`run_quantum`). That
      restored integration_test to 1055/0.

      BLOCKING GAP (why reverted): the synchronous runtime-drive loop in
      `run_exprs_via_runtime` (NullExecutor; idle only services a timer deadline,
      errors on `inbox_wakeup_required`) cannot service genuine async/concurrent
      I/O reached through the flipped `eval`/`eval_str`. Two production-async
      categories go RED (both green at baseline, neither fixable without the
      real executor / callback-ABI work of Tasks 04–06):
        1. HOF-callback async (1 test — `embedding_api_test::embedding_async_all_and_channels`):
           `(foldl + 0 (async/all (map (fn (x) (async (* x x))) …)))` fails with
           "async yield outside of scheduler context". A stdlib HOF (`map`/`foldl`)
           whose callback spawns/awaits `async` re-enters synchronously and the
           async yield escapes the cooperative scheduler. This is exactly the
           Task 04 `NativeOutcome::Call` callback-re-entry migration target.
        2. Concurrent external blocking I/O (3 tests — `mcp_async_test`:
           `cross_connection_overlap_proves_no_serialization`,
           `scheduler_not_stalled_sibling_completes_before_slow_call`,
           `cancellation_tombstones_connection_and_interpreter_stays_healthy`):
           `async/spawn`ed tasks that make blocking `mcp/call`s do NOT truly
           overlap on the NullExecutor sync-drive path (observed
           `["a-timed-out-without-marker", "b-done"]` — no in-flight interleave),
           whereas the legacy `init_scheduler` path achieved real concurrency.
           Needs a real executor (Task 05/06) to run blocking leaf calls
           off-thread while siblings progress.
      Simpler async DOES flip cleanly (`embedding_async_works_on_vm`,
      `(await (async …))`, sync I/O, timers, multimethods, modules, dynamic
      context all green through the runtime). The 4 pre-existing vm_async_test RED
      did NOT resolve — they run through `common::eval` → `eval_str_compiled`,
      which was deliberately NOT flipped (flipping it broke 14 more async tests
      that suspend on channels/blocking-sleep/deadlock).
      DECISION: reverted both changes to the exact green baseline (eval_test
      1072/0, integration_test 1055/0, vm_async_test 4 pre-existing RED,
      embedding_api_test 14/0, mcp_async_test 8/0). The flip is unblocked once
      Task 04 (callback re-entry ABI) and Task 05/06 (real executor for blocking
      leaf I/O + concurrent scheduling) land; the `*_via_runtime` entry points
      remain available for incremental validation._

- [ ] **Step 3: Remove TLS scheduler ownership**

Delete task vectors, IDs, virtual clock, and global env ownership from
`scheduler.rs`. After Task 4's ABI migration, delete signal/resume TLS,
`call_run_scheduler*`, scheduler target loops, and TLS spawn/cancel callbacks,
except the exact `AwaitIo` signal functions retained solely by
`LegacyAwaitIoBridge` for Tasks 05–08.
Any temporary Task 04 adapter is the one-way `LegacyAsyncAbiAdapter` defined in
Task 4: it receives task/runtime operations through `NativeCallContext`, owns no
scheduler state, is named in the inventory, and carries a Task 04 deletion
owner.

- [ ] **Step 4: Implement explicit interpreter teardown**

`Interpreter::drop` first takes `runtime` from its `Option`, calls nonblocking,
idempotent `close_for_interpreter_drop`, and drops the runtime. That operation
rejects admission, closes/detaches the executor lease while the inbox still
exists, requests cancellation, and removes all runtime-local VM, continuation,
decoder, task-context, output, and safely removable resource edges. It never
invokes user continuations or runs an unbounded shutdown loop from `Drop`.
Admitted dispatches may finish only against their send-safe terminal sink.

After runtime destruction, clear every `EvalContext` value store, release the
interpreter's `global_env` strong reference, and finally run the
`InterpreterDrop` collection. During unwinding, perform containment and edge
release but skip collection. Close the completion inbox only after executor
detach/lease shutdown can no longer start new submissions. Add counted drop
tests proving this order and proving no delivery accesses interpreter state
after drop begins.

- [ ] **Step 5: Run root and scheduler characterizations**

```bash
cargo test -p sema-lang --test runtime_roots_test
cargo test -p sema-lang --test vm_async_test -- scheduler
cargo test -p sema-lang --test unified_runtime_watchdog_test
cargo check -p sema-dap -p sema-mcp -p sema-wasm
```

Expected: root/fairness/watchdog cases pass. Exact Task 04 language-combinator
RED cases may remain. Unmigrated resource, LLM, and MCP producers still run
through the named `LegacyAwaitIoBridge`/`LegacyRuntimeBridge` and remain listed
with deletion owners in Tasks 05, 06, and 07/08 as applicable; they are not
silently treated as Task 04 RED cases.

## Task 7: Integrate debugger stop-the-world behavior

**Files:** `runtime/debug.rs`, `debug.rs`, `debug_session.rs`

- [ ] **Step 1: Write failing two-root debug tests**

When A hits a breakpoint, B must not execute until resume. Inspecting A and B is
read-only. Resume continues round-robin scheduling. Cancelling A while stopped
does not settle B.

- [ ] **Step 2: Bind debug state to `Runtime`**

Return `DriveState::DebugStopped`; do not create a debugger-only run loop.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-dap
cargo test -p sema-eval debug_session
```

Expected: existing debugger tests plus new multi-root tests pass.

## Task 8: Verify and independently review the layer

- [ ] **Step 1: Run layer gates**

```bash
cargo test -p sema-core
cargo test -p sema-vm
cargo test -p sema-eval
cargo test -p sema-lang --test runtime_roots_test
cargo test -p sema-lang --test vm_async_test
cargo test -p sema-lang --test runtime_conformance_test
cargo test -p sema-lang --test unified_runtime_watchdog_test
cargo fmt --all -- --check
cargo clippy -p sema-vm -p sema-eval --all-targets -- -D warnings
scripts/check-unified-runtime-legacy.sh > /tmp/runtime-legacy.actual
diff -u docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline /tmp/runtime-legacy.actual
rg -n 'set_yield_signal|take_yield_signal|set_resume_value|take_resume_value|call_run_scheduler|SchedulerTarget|SchedulerRunResult' crates --glob '*.rs'
git diff --check
```

Expected: all scheduler/root/VM/capture oracles owned by Task 03 are GREEN.
Only exact Task 04 language-contract RED cases may remain and must be itemized.
The legacy-symbol search may find only the explicitly inventoried
`LegacyAsyncAbiAdapter` and exact `LegacyAwaitIoBridge` producer list. It finds
no scheduler owner, recursive drive callback, promise/channel/sleep signal, or
resume-value slot used by the new runtime.

- [ ] **Step 2: Record evidence and assign independent review**

The report records queue traces, state counts before/after shutdown, stale
completion counters, GC results, and remaining adapters. Reviewer finding IDs
use `UR-T03-R###`. Review must trace at least one root, detached task, timer,
external completion, cancellation, breakpoint, and interpreter drop from
creation through reaping.

- [ ] **Step 3: Fix each finding test-first and rerun full gates**

No finding is waived because it is timing-sensitive. Replace nondeterministic
reproduction with the fake clock or bounded drive harness.

- [ ] **Step 4: Commit the accepted layer**

```bash
git add crates/sema-core crates/sema-vm crates/sema-eval \
  crates/sema/tests docs/internals/async-runtime-inventory.md \
  docs/plans/evidence/unified-cooperative-runtime \
  docs/plans/reviews/unified-cooperative-runtime
git commit -m "refactor(runtime): install interpreter-owned scheduler"
```

## Completion criteria

- One interpreter owns exactly one runtime and supports multiple live roots.
- Root and within-root ordering match the exact fairness sequences.
- Every drive turn is deterministically bounded.
- Native calls suspend/resume through traced, exactly-once VM continuations.
- Wait generations reject wrong, late, and duplicate completions.
- Settlement order is monotonic and assigned before observation.
- Parent/child captured mutation is coherent and cycle-safe.
- Debug stops the interpreter, not an arbitrary subset of roots.
- The legacy scheduler owns no task or clock state.
- Independent review and durable evidence are clean.
