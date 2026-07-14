# Task 02: Core Runtime Data Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Define the checked identities, task relationships, settlements,
continuation protocol, task context, and worker-completion boundary consumed by
every later runtime layer, without changing scheduling behavior.

**Architecture:** `sema-core` owns vocabulary and traceable interfaces; it does
not own the scheduler. Runtime-thread values stay on the runtime thread. Worker
threads return `Send` payloads tagged with runtime, wait, generation, and
operation identities, and a runtime-thread decoder converts those payloads into
Sema values. Only interruptible and provably bounded resource classes can be
constructed.

**Tech Stack:** Rust 2021, `sema-core`, CORE-2 tracing, Cargo tests, Clippy.

## Execution contract

- **Status:** Ready only after Task 01 is accepted and committed.
- **Dependencies:** Task 01 tests, inventory, scanner, baseline, evidence, review.
- **Immutable inputs:** Master sections “Independent task relationships,”
  “Settlement,” “Worker-thread boundary,” “Resource cancellation classes,” and
  “Task context and inheritance.”
- **Exact start state:** Clean worktree; `git log -1 --format=%s` is
  `test(runtime): lock unified runtime contracts`; Task 01 gates match evidence.
- **Parallel work:** IDs/relations, completion/resource, and task-context test
  work may proceed in separate files. `value.rs`, `cycle.rs`, `error.rs`, public
  exports, inventory, and baseline have one integration owner; review starts
  only after their merge.

## Global constraints

- Read `AGENTS.md`, the master runtime specification, Task 01 evidence, and
  `docs/plans/2026-07-02-core2-gc.md` before editing.
- Run Task 01 first. Its legacy scanner and characterization suite are hard
  gates throughout this task.
- Change types and adapters only. Do not add ready queues, timers, root driving,
  or new language-facing async behavior.
- Preserve the single-threaded `Rc` runtime model. `Value`, `Env`, VM frames,
  continuations, and `SemaError` never cross a worker-thread boundary.
- Do not add an `Unbounded`, `Uninterruptible`, or catch-all resource class.
- Do not profile or benchmark this layer.

---

## Files and responsibilities

**Create**

- `crates/sema-core/src/runtime/mod.rs` — exports and module-level invariants.
- `crates/sema-core/src/runtime/ids.rs` — checked monotonic identifiers.
- `crates/sema-core/src/runtime/cancel.rs` — cancellation ancestry and reasons.
- `crates/sema-core/src/runtime/settlement.rs` — ordered terminal outcomes.
- `crates/sema-core/src/runtime/completion.rs` — send-only completion envelope.
- `crates/sema-core/src/runtime/task_context.rs` — explicit inherited context.
- `crates/sema-core/src/runtime/native.rs` — return/call/suspend continuations.
- `crates/sema-core/src/runtime/resource.rs` — constructible resource policies.
- `crates/sema-core/tests/runtime_types_test.rs` — public contract tests.
- `docs/plans/evidence/unified-cooperative-runtime/task-02.md` — verification
  transcript and remaining RED characterization list.
- `docs/plans/reviews/unified-cooperative-runtime/task-02.md` — independent
  review findings and disposition.

**Modify**

- `crates/sema-core/src/lib.rs` — export `runtime`.
- `crates/sema-core/src/error.rs` — structured timeout/cancellation conditions.
- `crates/sema-core/src/context.rs` — expose a task-context handle through the
  evaluation context without duplicating ambient state.
- `crates/sema-core/src/value.rs` — add native-result adapters and trace hooks;
  leave existing call sites source-compatible in this task.
- `crates/sema-core/src/cycle.rs` — trace continuation and task-context values.
- `crates/sema-core/src/async_signal.rs` — bridge legacy task identifiers and
  yield results to the new types, with deletion ownership recorded.
- `docs/internals/async-runtime-inventory.md` — mark each bridge as temporary and
  assign deletion to Task 03 or Task 08.
- `docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline` —
  update only for intentional type names or adapters.

## Exact public interfaces

Implement these names and meanings. A reviewer must reject aliases whose fields
erase one of the identities or ownership axes.

```rust
pub struct RuntimeId(NonZeroU64);
pub struct RootId(NonZeroU64);
pub struct TaskId(NonZeroU64);
pub struct ScopeId(NonZeroU64);
pub struct PromiseId(NonZeroU64);
pub struct ChannelId(NonZeroU64);
pub struct WaitId(NonZeroU64);
pub struct WaitGeneration(NonZeroU64);
pub struct OperationId(NonZeroU64);
pub struct SettlementSeq(NonZeroU64);

pub struct IdCounter<I> { /* private next value and marker */ }
impl<I> IdCounter<I> {
    pub fn allocate(&mut self) -> Result<I, IdExhausted>;
}
```

All identity types implement `Copy`, `Clone`, `Debug`, `Eq`, `Ord`, and `Hash`.
Construction is crate-controlled. Allocation starts at one, uses checked
increment, never wraps, and permanently reports `IdExhausted` after exhaustion.

```rust
pub enum CancellationParent {
    Root(RootId),
    Task(TaskId),
    Scope(ScopeId),
}

pub enum LifetimeOwner {
    Interpreter,
    Scope(ScopeId),
}

pub struct TaskRelations {
    pub origin_root: RootId,
    pub cancellation_parent: CancellationParent,
    pub lifetime_owner: LifetimeOwner,
}

pub enum CancelReason {
    Explicit { message: Option<String> },
    RootCancelled { root: RootId },
    OwnerCancelled { scope: ScopeId },
    ScopeFailed { scope: ScopeId },
    Timeout { operation: &'static str, duration_ms: u64 },
    HostStopped { root: RootId },
    ResourceDisconnected { operation: OperationId },
    InterpreterShutdown,
    HostShutdown,
}
```

Observation is deliberately absent from `TaskRelations`: promise handles and
wait registrations represent observers; they do not own or parent tasks.

```rust
pub enum TaskOutcome {
    Returned(Value),
    Failed(SemaError),
    Cancelled(CancelReason),
}

pub struct TaskSettlement {
    pub sequence: SettlementSeq,
    pub outcome: TaskOutcome,
}
```

`TaskOutcome::Cancelled` is a distinct internal state. Converting it to a
language condition happens only at an evaluation boundary.

```rust
pub type SendPayload = Box<dyn Any + Send>;

pub struct CompletionKind(NonZeroU16);

pub struct ExternalCompletion {
    pub runtime_id: RuntimeId,
    pub wait_id: WaitId,
    pub generation: WaitGeneration,
    pub operation_id: OperationId,
    pub kind: CompletionKind,
    pub result: Result<SendPayload, ExternalFailure>,
}

pub enum ExternalFailureCode {
    Rejected,
    Cancelled,
    DeadlineExceeded,
    BoundExceeded,
    WorkerPanic,
    Decode,
}

pub struct ExternalFailure {
    pub code: ExternalFailureCode,
    pub message: String,
    pub source: Option<String>,
}

pub trait CompletionDecoder {
    fn decode(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> NativeResult;
}
```

The envelope cannot contain `Value`, `Env`, `SemaError`, `Rc`, or a VM
continuation. `ExternalFailure` is a send-safe code/message/source structure,
not an erased `SemaError`. `CompletionKind` is a crate-controlled, non-zero,
send-safe discriminator. The wait registry stores the expected kind and rejects
a wrong-kind completion before invoking its decoder. A payload that has the
correct declared kind but fails the decoder's concrete downcast becomes a named
`Decode` failure for that operation; wrong-kind delivery never changes the
waiting task's outcome.

The interpreter runtime selects `CompletionKind` while registering the wait and
copies it into the private `CompletionSink` defined in Task 05. A worker job can
deliver only its send-safe result through that sink; it cannot select a kind,
forge identity fields, or return a runtime-side decoder/resource hook.

```rust
pub type NativeResult = Result<NativeOutcome, SemaError>;

pub enum NativeOutcome {
    Return(Value),
    Call(NativeCall),
    Suspend(NativeSuspend),
}

pub struct NativeCall {
    pub callable: Value,
    pub arguments: Vec<Value>,
    pub continuation: Box<dyn NativeContinuation>,
}

pub struct NativeSuspend {
    pub wait: WaitRequest,
    pub continuation: Box<dyn NativeContinuation>,
}

pub struct WaitRequest {
    pub kind: WaitKind,
}

pub enum WaitKind {
    Timer(Duration),
    Promise(PromiseId),
    Channel(ChannelWait),
    External(ExternalWait),
}

pub enum ChannelWait {
    Send { channel: ChannelId, value: Value },
    Receive { channel: ChannelId },
}

pub struct ExternalWait {
    pub operation_id: OperationId,
    pub expected_kind: CompletionKind,
    pub decoder: Box<dyn CompletionDecoder>,
    pub resource: ResourceClassDescriptor,
}

pub enum ResumeInput {
    Returned(Value),
    Failed(SemaError),
    Cancelled(CancelReason),
    Completion(Result<SendPayload, ExternalFailure>),
}

pub trait NativeContinuation: Trace {
    fn resume(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult;
}
```

Continuation consumption enforces exactly-once resumption. `Trace` must visit
every stored `Value`; host-only state must obey CORE-2 and may not strongly
capture an `Env` or traceable object through an opaque closure.

```rust
pub enum ResourceClassDescriptor {
    Interruptible,
    QuarantinedBounded { bound: QuarantineBound },
}

pub enum ResourceClass {
    Interruptible { cancel: Box<dyn CancelHook> },
    QuarantinedBounded { bound: QuarantineBound },
}

pub enum QuarantineBound {
    HardDeadline(Duration),
    FiniteWork { kind: &'static str, maximum_units: NonZeroU64 },
}

pub trait CancelHook {
    fn cancel(&mut self) -> Result<(), CancelHookError>;
}
```

Construction rejects a zero deadline or zero work bound. A finite-work producer
must prove its unit count before dispatch and must not expand the bound while it
runs. Cancellation hooks are idempotent by contract and are tested with repeated
calls. The type has no third variant.

`TaskContext` contains named core fields for output routing, sandbox/VFS policy,
current file/module state, user-visible call-stack metadata, tracing, and usage
budget. Subsystem-specific state uses typed extension slots:

```rust
pub trait TaskLocalValue: Trace {
    fn inherit(&self) -> Rc<dyn TaskLocalValue>;
    fn as_any(&self) -> &dyn Any;
}
```

`TaskContext::inherit_for_child()` copies each core field according to the
master specification and calls `inherit()` for each registered extension. It
must not fall back to cloning the entire `EvalContext`.

`SemaError` gains constructors and structured condition data for cancellation
and timeout. Exact predicates exposed later are `:cancelled?` and `:timeout?`;
this task tests Rust condition metadata only.

---

## Task 1: Lock identity and relationship behavior

**Files:** `runtime/ids.rs`, `runtime/cancel.rs`, `runtime_types_test.rs`

- [ ] **Step 1: Write failing identity tests**

Test independent counters, stable ordering, non-zero allocation, exhaustion
without wrap, and formatting that includes the identity kind. Add compile-time
trait assertions for the required identity traits.

- [ ] **Step 2: Write relationship tests**

Construct these exact cases and assert all three axes independently:

- a root task owned by its interpreter;
- a detached child with `origin_root = A`, parent task in A, interpreter owner;
- a scoped child originating in root B, scope parent, scope owner.

- [ ] **Step 3: Implement the minimum types and run**

```bash
cargo test -p sema-core --test runtime_types_test -- ids
cargo test -p sema-core --test runtime_types_test -- relationships
```

Expected: all selected tests pass; no scheduler or TLS file is needed by the
test.

## Task 2: Lock settlement and condition behavior

**Files:** `runtime/settlement.rs`, `error.rs`, `runtime_types_test.rs`

- [ ] **Step 1: Write failing outcome tests**

Assert `Returned`, `Failed`, and `Cancelled` remain distinguishable, settlement
sequence ordering is independent of task ID, and conversion of cancellation to
a condition preserves reason metadata.

- [ ] **Step 2: Implement outcomes and condition constructors**

Do not make cancellation an ordinary error inside `TaskOutcome`. Do not assign
sequence numbers in this module; Task 03 owns the runtime counter.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-core --test runtime_types_test -- settlement
cargo test -p sema-core --test runtime_types_test -- condition
```

Expected: selected tests pass, including structured reason round trips.

## Task 3: Enforce the worker boundary and resource classes

**Files:** `runtime/completion.rs`, `runtime/resource.rs`,
`runtime_types_test.rs`

- [ ] **Step 1: Write failing compile and behavior tests**

Use `static_assertions` already available in the workspace, or a small local
`fn assert_send<T: Send>()`, to prove `ExternalCompletion: Send`. Test that a
correct-kind payload with the wrong concrete type becomes a named decode error;
Task 03 tests that a wrong declared kind is discarded before decoding and leaves
the task unchanged. Test interrupt cancellation twice and bounded-resource
zero-deadline/zero-work rejection. Also prove the decoder and concrete
`ResourceClass` remain runtime-side; Task 05 separately proves only the
`Box<dyn IoJob>` and private completion sink cross to the pool.

- [ ] **Step 2: Implement the envelope and policies**

Keep decoder storage on the runtime side; only the completion envelope is sent.
Use explicit fields rather than a tuple key so stale-completion diagnostics can
name every mismatch.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-core --test runtime_types_test -- completion
cargo test -p sema-core --test runtime_types_test -- resource
```

Expected: selected tests pass; `ResourceClass` has exactly two variants.

## Task 4: Define traceable native continuations

**Files:** `runtime/native.rs`, `value.rs`, `cycle.rs`,
`runtime_types_test.rs`

- [ ] **Step 1: Add failing continuation tests**

Cover return, tail call request, suspension request, each `ResumeInput`, a
continuation that stores a `Value`, and exactly-once consumption. Add a CORE-2
test proving stored values are visited by tracing.

- [ ] **Step 2: Implement outcomes and compatibility constructors**

Add `NativeFn::simple_result` and `NativeFn::with_context_result` constructors
returning `NativeResult`. Preserve `NativeFn::simple` and `NativeFn::with_ctx`
as adapters producing `NativeOutcome::Return`; record their Task 08 deletion or
retention decision in the inventory.

- [ ] **Step 3: Run core and cycle tests**

```bash
cargo test -p sema-core --test runtime_types_test -- native
cargo test -p sema-core cycle
```

Expected: all selected tests pass and no opaque continuation capture bypasses
tracing.

## Task 5: Make task-context inheritance explicit

**Files:** `runtime/task_context.rs`, `context.rs`, `cycle.rs`,
`runtime_types_test.rs`

- [ ] **Step 1: Write a table-driven inheritance test**

The table has one row per core field with columns `field`, `share/copy/reset`,
`child assertion`, and `mutation visibility`. Include a custom traced extension
whose `inherit()` call count is observable.

- [ ] **Step 2: Implement `TaskContext` and the evaluation handle**

Keep named fields private and expose policy-specific accessors. A missing field
policy is a test failure; do not use `#[non_exhaustive]` to hide incomplete
coverage.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-core --test runtime_types_test -- task_context
cargo test -p sema-core context
cargo test -p sema-core cycle
```

Expected: field-by-field table and tracing tests pass.

## Task 6: Add temporary legacy bridges and source guards

**Files:** `async_signal.rs`, `value.rs`, inventory, legacy baseline

- [ ] **Step 1: Name every bridge with `LegacyRuntimeBridge`**

Bridge current scheduler callback/task IDs to checked IDs without inventing a
second semantic source. Add an inventory row with exact symbol, caller list,
replacement, and deletion task for every bridge.

- [ ] **Step 2: Refresh and inspect the baseline**

```bash
scripts/check-unified-runtime-legacy.sh > /tmp/runtime-legacy.actual
diff -u docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline /tmp/runtime-legacy.actual
```

If intentional additions appear, inspect every line, update the committed
baseline, and state the reason in Task 02 evidence. Unexpected production paths
must be removed.

- [ ] **Step 3: Prove behavior has not switched yet**

```bash
cargo test -p sema-lang --test vm_async_test
cargo test -p sema-lang --test runtime_conformance_test
```

Expected: the same Task 01 RED characterization cases remain RED for the same
reasons; previously GREEN cases stay GREEN. Record exact names and outcomes.

## Task 7: Verify and independently review the layer

- [ ] **Step 1: Run focused and workspace-quality gates**

```bash
cargo test -p sema-core
cargo test -p sema-lang --test runtime_conformance_test
cargo fmt --all -- --check
cargo clippy -p sema-core --all-targets -- -D warnings
git diff --check
```

Expected: all core/type/guard tests and formatting checks pass. Known Task 01
behavioral RED cases are documented separately and are not part of a command
claimed as GREEN.

- [ ] **Step 2: Write durable evidence**

Record commands, exit status, relevant counts, the exact remaining RED tests,
and baseline changes in `task-02.md`. Do not paste terminal color codes or rely
on `/tmp` artifacts.

- [ ] **Step 3: Assign independent review**

The reviewer checks:

- all IDs are checked and cannot alias through raw integers;
- observation, cancellation ancestry, and lifetime ownership remain separate;
- no runtime-thread object crosses the send boundary;
- every continuation-held value is traced;
- there is no constructible unbounded resource class;
- every compatibility bridge has a deletion owner.

Write stable finding IDs `UR-T02-R###`, severity, file/line evidence,
reproduction, and disposition to `task-02.md` in the reviews directory.

- [ ] **Step 4: Fix findings and rerun affected plus full gates**

Every defect gets a failing regression test before its fix. The implementer may
not close their own finding without reviewer confirmation.

- [ ] **Step 5: Commit the accepted layer**

```bash
git add crates/sema-core docs/internals/async-runtime-inventory.md \
  docs/plans/evidence/unified-cooperative-runtime \
  docs/plans/reviews/unified-cooperative-runtime
git commit -m "refactor(runtime): define core runtime contracts"
```

## Completion criteria

- Every exact interface above exists and is covered by a public contract test.
- Runtime identities cannot wrap or silently collide.
- Settlements preserve value, error, and cancellation as distinct outcomes.
- External completions are send-safe and contain no runtime-thread values.
- Native continuation state is exactly-once and fully traced.
- Task-context inheritance is explicit field by field.
- Only interruptible and quarantined-bounded resources are constructible.
- Legacy behavior has not switched, and every temporary bridge has a deletion
  task.
- Independent review is clean and evidence is committed.
