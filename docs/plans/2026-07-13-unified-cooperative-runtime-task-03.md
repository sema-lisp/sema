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
- **Exact start state:** Clean worktree; latest commit subject is
  `refactor(runtime): define core runtime contracts`; Task 01–02 gates are GREEN
  except the exact Task 03/04 RED cases recorded in Task 02 evidence.
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
    pub context: TaskContext,
    pub output: Rc<dyn OutputSink>,
}

pub struct PreparedRoot {
    pub vm: VM,
    pub entry: Value,
}

#[derive(Clone)]
pub struct RootHandle {
    runtime: Weak<RefCell<RuntimeState>>,
    id: RootId,
}

pub struct DriveBudget {
    pub completion_limit: NonZeroUsize,
    pub timer_limit: NonZeroUsize,
    pub root_visit_limit: NonZeroUsize,
    pub cleanup_limit: NonZeroUsize,
    pub instruction_limit_per_task: NonZeroUsize,
    pub wall_clock_limit: Duration,
}

pub enum DriveState {
    Progress { work_items: usize, ready_remaining: bool },
    Idle {
        next_deadline: Option<Instant>,
        inbox_wakeup_required: bool,
    },
    Quiescent,
    DebugStopped(DebugStop),
    ShutdownComplete(ShutdownReport),
}

pub struct ShutdownOptions {
    pub deadline: Instant,
    pub drive_budget: DriveBudget,
}

impl Runtime {
    pub fn new(clock: Rc<dyn RuntimeClock>) -> Self;
    pub fn submit_root(
        &self,
        prepared: PreparedRoot,
        options: VmRootOptions,
    ) -> RootHandle;
    pub fn drive(&self, budget: DriveBudget) -> DriveState;
    pub fn cancel_root(&self, root: RootId, reason: CancelReason) -> bool;
    pub fn shutdown(&self, options: ShutdownOptions) -> ShutdownReport;
}

impl RootHandle {
    pub fn id(&self) -> RootId;
    pub fn poll_result(&self) -> RootPoll;
    pub fn cancel(&self, reason: CancelReason) -> bool;
}
```

`RuntimeState` is private. `Runtime` is the interpreter-owned strong handle and
is not `Clone`; root handles retain only `Weak` access to the same state. This
internal-handle shape lets `submit_root(&self, ...)` construct a non-owning
`RootHandle` without requiring an impossible `Weak` conversion from `&mut
Runtime` or introducing a second runtime owner.

`RootPoll` is `Pending`, `Ready(Result<Value, SemaError>)`, or
`RuntimeDropped`. Polling never drives the runtime. Result retrieval is
idempotent and never consumes the stored settlement.

```rust
pub enum TaskState {
    Ready,
    Running,
    Waiting {
        wait_id: WaitId,
        generation: WaitGeneration,
    },
    Settled(TaskSettlement),
}

pub enum VmExecResult {
    Finished(Value),
    Failed(SemaError),
    Native(NativeOutcome),
    QuantumExpired,
    DebugStopped(DebugStop),
}

pub struct CancellationRequest {
    pub reason: CancelReason,
}
```

All state transitions go through named methods that reject illegal edges in
debug and test builds. `Running -> Running`, `Settled -> Ready`, and any other
transition out of `Settled` are defects. Cleanup progress is side state on the
task/cleanup registry rather than a lifecycle variant, and reaping removes a
settled record from `TaskStore`; neither changes the master lifecycle. Each task
stores `Option<CancellationRequest>` separately from `TaskState`; setting it is
idempotent and does not prematurely turn a task awaiting cleanup into settled.

`ReadyRoots` stores one FIFO `VecDeque<TaskId>` per `RootId` plus a rotating
`VecDeque<RootId>`. Enqueuing a second ready task for an already-present root
does not enqueue the root twice. After one task quantum, a still-runnable root
moves to the back. Within that root, a yielded task moves behind tasks already
ready there.

Wait lookup uses `(WaitId, WaitGeneration)`. Each registration also stores its
`OperationId`, decoder, cancellation policy, and waiting task. Cancellation or
completion removes the registration before invoking callbacks. A late or
duplicate completion is counted and discarded; it never resumes another wait.

The runtime assigns `SettlementSeq` at the single transition into `Settled`.
Sequence assignment occurs before waking observers, so pre-settled promise races
can be ordered later without list-order bias.

## Task 1: Implement root/task state machines from tests

**Files:** `runtime/root.rs`, `runtime/task.rs`, `runtime/tests.rs`

- [ ] **Step 1: Write failing transition-table tests**

Test every legal edge and representative illegal edges. Include returned,
failed, and cancelled settlements and prove the settlement sequence is assigned
once. Test that origin root, cancellation parent, and lifetime owner never
change when a task changes state.

- [ ] **Step 2: Implement the records and transition methods**

Use checked IDs from Task 02. Do not expose mutable task/root fields outside the
runtime module.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-vm runtime::tests::state
```

Expected: transition table passes; invalid edges fail with the named old/new
states.

## Task 2: Implement deterministic fair ready queues

**Files:** `runtime/ready.rs`, `runtime/tests.rs`

- [ ] **Step 1: Write failing sequence tests**

Assert exact dequeue sequences for:

- roots A/B/C with one perpetually requeued task each: `A B C A B C`;
- root A with tasks A1/A2/A3 and root B with B1: `A1 B1 A2 B1 A3 B1`;
- removal of a settled root without disturbing remaining order;
- duplicate wake attempts for one task.

- [ ] **Step 2: Implement queue invariants and duplicate protection**

Keep membership sets private and assert queue/set agreement after mutations in
test builds.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-vm runtime::tests::ready
```

Expected: exact order assertions pass.

## Task 3: Implement timers, completions, and stale-delivery rejection

**Files:** `runtime/timer.rs`, `runtime/wait.rs`, `runtime/drive.rs`,
`runtime/tests.rs`

- [ ] **Step 1: Add a fake monotonic clock and failing tests**

Cover same-deadline insertion order, zero duration, timer cancellation,
generation reuse, wrong runtime, wrong operation, wrong completion kind,
correct-kind payload decode failure, duplicate completion,
completion-vs-cancellation races, per-turn completion/timer/cleanup/root limits,
and wall-clock budget expiry measured with an injected clock. Wrong-kind
delivery must leave the wait/task outcome unchanged and must not invoke the
decoder; correct-kind decode failure is delivered as the Task 02 `Decode`
failure.

- [ ] **Step 2: Implement registration and bounded drains**

Do not sleep in `Runtime::drive`. Return `Idle { next_deadline }`; native hosts
may wait outside the runtime and browser hosts may schedule a macrotask.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-vm runtime::tests::timer
cargo test -p sema-vm runtime::tests::wait
cargo test -p sema-vm runtime::tests::drive_limits
```

Expected: deterministic tests pass without wall-clock delays.

## Task 4: Make the VM resumable through explicit frames

**Files:** `vm.rs`, `debug.rs`, `runtime/task.rs`, `runtime/tests.rs`

- [ ] **Step 1: Write failing VM continuation tests**

Test native return, native call into a Sema closure, suspend/resume with each
outcome, nested native-to-Sema-to-native calls, exception propagation across a
continuation, cancellation at a safe point, and repeated quantum expiry.

- [ ] **Step 2: Add explicit continuation frames**

Store them in traceable VM/task state. Remove the `set_yield_signal` plus dummy
`nil` protocol from the VM dispatch path. A native call returns
`VmExecResult::Native`; the runtime executes or registers it, then resumes the
same VM through an explicit frame.

- [ ] **Step 3: Add instruction budgeting**

Check the budget at dispatch safe points and return `QuantumExpired` without
losing stack, open-upvalue, handler, or debug state. Zero-work spinning inside a
native continuation is forbidden: each continuation transition consumes a
drive work item.

- [ ] **Step 4: Run focused VM suites**

```bash
cargo test -p sema-vm runtime::tests::continuation
cargo test -p sema-vm runtime::tests::quantum
cargo test -p sema-lang --test vm_integration_test
```

Expected: all selected tests pass and no nested scheduler entry occurs.

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
`scheduler.rs`, `async_signal.rs`, `runtime_roots_test.rs`

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

- [ ] **Step 3: Remove TLS scheduler ownership**

Delete task vectors, IDs, virtual clock, and global env ownership from
`scheduler.rs`. Any temporary function kept for Task 04 must delegate to the
active interpreter runtime through `NativeCallContext`, be named in the
inventory, and carry a Task 04 deletion owner.

- [ ] **Step 4: Run root and scheduler characterizations**

```bash
cargo test -p sema-lang --test runtime_roots_test
cargo test -p sema-lang --test vm_async_test -- scheduler
cargo test -p sema-lang --test unified_runtime_watchdog_test
```

Expected: root/fairness/watchdog cases pass; language-combinator cases assigned
to Task 04 may remain RED and are listed by exact name in evidence.

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
git diff --check
```

Expected: all scheduler/root/VM/capture oracles owned by Task 03 are GREEN.
Only exact Task 04 language-contract RED cases may remain and must be itemized.

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
