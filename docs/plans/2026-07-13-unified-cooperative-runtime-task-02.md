# Task 02: Core Runtime Data Model Implementation Plan

> **For agentic workers:** Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans`. Check off each step as it is completed.

**Goal:** Add compile-realistic core identities, native suspension vocabulary,
task-local extension storage, structured conditions, tracing contracts, and the
executor admission seam without changing production scheduling or I/O behavior.

**Architecture:** `sema-core::runtime` owns types, invariants, and opaque
ownership wrappers, not queues or a scheduler. Runtime-thread objects remain
`Rc`-local. Executor work and completion payloads are `Send`; only private core
wrappers own completion authority. The existing public `IoBackend` API remains
unchanged until Task 05.

## Execution contract

- **Status:** Ready when Task 01 acceptance commit `be984860` is an ancestor of
  `HEAD`. Later chronology/documentation commits are allowed.
- **Start check:** the worktree is clean and
  `git merge-base --is-ancestor be984860 HEAD` succeeds. Do not require a
  particular latest commit subject.
- **Dependencies:** Task 01 inventory, scanner, baseline, evidence, and review.
- **Scope:** types and compatibility adapters only. No queue, timer, drive,
  wait-registration, rollback transaction, evaluator-call execution, output routing,
  production `sema-io`, or WASM host implementation.
- **Parallel work:** IDs/relations, completion/resource, and context-shell tests
  may proceed independently. `value.rs`, `cycle.rs`, `error.rs`, exports,
  inventory, and baseline have one integration owner.

## Files and responsibilities

**Create**

- `crates/sema-core/src/runtime/{mod,ids,cancel,settlement,completion,executor,resource,native,task_context,trace}.rs`
- `crates/sema-core/tests/runtime_types_test.rs` for public API behavior only.
- `docs/plans/evidence/unified-cooperative-runtime/task-02.md` during execution,
  using the existing evidence convention (commands, results, remaining REDs).
- `docs/plans/reviews/unified-cooperative-runtime/task-02.md` after independent
  review.

**Modify**

- `crates/sema-core/src/lib.rs` to export `runtime`.
- `crates/sema-core/src/value.rs` to preserve the current `NativeFn.func` ABI
  while adding its private runtime-aware path and constructors.
- `crates/sema-core/src/context.rs` to store an optional `TaskContextHandle` and
  initialize it in every constructor; do not move existing fields.
- `crates/sema-core/src/error.rs` for condition constructors and condition types.
- `crates/sema-core/src/cycle.rs` only for actual opaque-payload trace delegation.
- `crates/sema-core/src/async_signal.rs` for named, fallible legacy conversions;
  raw callback and promise signatures remain unchanged.
- `docs/internals/async-runtime-inventory.md` and the legacy baseline for exact
  bridge ownership.

Do **not** modify `io_backend.rs` in Task 02. `IoBackend` is an additive legacy
surface and remains public and behavior-compatible through Task 05.

## Checked identities

```rust
pub struct RuntimeId(NonZeroU64);
pub struct RootId { runtime: RuntimeId, local: NonZeroU64 }
pub struct TaskId(NonZeroU64);
pub struct ScopeId(NonZeroU64);
pub struct PromiseId { runtime: RuntimeId, local: NonZeroU64 }
pub struct ChannelId { runtime: RuntimeId, local: NonZeroU64 }
pub struct WaitId(NonZeroU64);
pub struct WaitGeneration(NonZeroU64);
pub struct OperationId(NonZeroU64);
pub struct SettlementSeq(NonZeroU64);
pub struct CompletionKind(NonZeroU64);

pub struct IdCounter<I> { next: Option<NonZeroU64>, /* marker */ }
pub struct RuntimeScopedIdCounter<I> {
    runtime: RuntimeId,
    local: IdCounter<NonZeroU64>,
    /* marker */
}
```

`RuntimeId` uses a process-global checked atomic allocator. Scalar runtime-local
IDs use `IdCounter`; `RootId`, `PromiseId`, and `ChannelId` use
`RuntimeScopedIdCounter`. Allocation starts at one. `next: None` means exhausted;
after allocating `u64::MAX`, every later allocation returns `IdExhausted` and no
counter wraps.

All IDs implement `Copy`, `Clone`, `Debug`, `Eq`, `Ord`, and `Hash`, with
read-only `get()` accessors; scoped IDs also expose `runtime()` and `local()`.
Only cross-crate legacy inputs have public raw construction:

```rust
impl TaskId {
    pub fn try_from_raw(raw: u64) -> Result<Self, InvalidRuntimeId>;
}
impl CompletionKind {
    pub fn try_from_raw(raw: u64) -> Result<Self, InvalidRuntimeId>;
}
```

`CompletionKind` is selected per wait, not globally allocated. Exhaustion
injection remains a crate-private unit-test seam. Construction/exhaustion tests
belong in `runtime` module unit tests; public integration tests exercise only
the public API. No impossible zero-`NonZeroU64` fixture is required.

Task relationships and settlements remain:

```rust
pub struct TaskRelations {
    pub origin_root: RootId,
    pub cancellation_parent: CancellationParent,
    pub lifetime_owner: LifetimeOwner,
}
pub enum TaskOutcome { Returned(Value), Failed(SemaError), Cancelled(CancelReason) }
pub struct TaskSettlement { pub sequence: SettlementSeq, pub outcome: TaskOutcome }
```

Observation is neither cancellation ancestry nor lifetime ownership. Retention
rules for roots, promises, and channels are deferred to Tasks 03 and 04; Task 02
defines IDs only.

## Completion and decoding boundary

Task 3 below first adds the compile prerequisite shared by completion and
prepared operations: `Trace`, an opaque `TaskContext` declaration,
`CancellationView`/`NativeCallContext`, `DecodedCompletion`, and
`CompletionDecoder`. This order is required:
the decoder is traceable and names both native types, while
`PreparedExternalOperation` owns a decoder. It does not add native outcomes,
continuations, the `NativeFn` dual ABI, or tracing behavior; those remain Task 4.

```rust
pub type SendPayload = Box<dyn Any + Send>;

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

pub fn downcast_send_payload<T: Any + Send>(
    payload: SendPayload,
    operation: &'static str,
) -> Result<T, ExternalFailure>;
```

The helper returns an `ExternalFailureCode::Decode` failure naming `operation`
and the expected Rust type when downcast fails. The envelope cannot contain
`Value`, `Env`, `SemaError`, `Rc`, a continuation, or other runtime-thread state.
Wrong-kind rejection before decode is Task 03 behavior.

`DecodedCompletion` is the decoder's native-independent terminal value:

```rust
pub type DecodedCompletion = Result<Value, SemaError>;
```

`CompletionDecoder` consumes one correctly routed result and runs on the
runtime thread:

```rust
pub trait CompletionDecoder: Trace {
    fn decode(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion;
}
```

## Executor seam

The new seam is `sema_core::runtime::executor`. Rust has no friend-crate
visibility. `CompletionSink`, its constructor, `complete`, submission
construction, and their fields therefore remain plain private inside
`sema_core::runtime`; downstream crates cannot name the sink.

```rust
pub trait CompletionSender: Send + Sync + 'static {
    // Bounded and non-panicking; it never waits for inbox capacity.
    fn send(&self, completion: ExternalCompletion) -> CompletionDelivery;
}

#[doc(hidden)]
pub struct CompletionRegistrar { /* private runtime id + sender capability */ }

impl CompletionRegistrar {
    /// Allocates a fresh RuntimeId; it cannot target an existing runtime.
    #[doc(hidden)]
    pub fn register(sender: Arc<dyn CompletionSender>) -> (RuntimeId, Self);

    #[doc(hidden)]
    pub fn bind(
        &self,
        identity: RuntimeIssuedCompletionIdentity,
        prepared: PreparedExternalOperation,
    ) -> ExternalOperationBinding;
}

pub struct ExternalOperationBinding { /* private runtime-local + dispatch halves */ }

impl ExternalOperationBinding {
    #[doc(hidden)]
    pub fn split(self) -> (RuntimeOperationBinding, ExecutorSubmission);
}

pub struct ExecutorSubmission { /* private sink, token, job, identity */ }

impl ExecutorSubmission {
    pub fn operation_id(&self) -> OperationId;
    pub fn into_dispatch(self) -> ExecutorDispatch;
    pub fn reject(self, kind: SubmitErrorKind) -> SubmissionRejected;
}

pub enum ExecutorDispatch {
    Async(AsyncExecutorDispatch),
    Blocking(BlockingExecutorDispatch),
}

impl AsyncExecutorDispatch {
    pub fn operation_id(&self) -> OperationId;
    pub fn into_future(self) -> AsyncDispatchFuture;
}

impl Future for AsyncDispatchFuture {
    type Output = ExecutorDriveReport;
}

impl BlockingExecutorDispatch {
    pub fn operation_id(&self) -> OperationId;
    pub fn class(&self) -> BlockingDispatchClass;
    pub fn run(self) -> ExecutorDriveReport;
}

pub enum ExecutorTerminal { Completed, Cancelled, WorkerPanic }
pub struct ExecutorDriveReport {
    pub terminal: ExecutorTerminal,
    pub delivery: CompletionDelivery,
}

pub struct SubmissionRejected { /* rejection kind + rejected submission */ }

impl SubmissionRejected {
    pub fn kind(&self) -> SubmitErrorKind;
    pub fn operation_id(&self) -> OperationId;
    pub fn rollback(self) -> SubmitErrorKind;
}

pub enum ExecutorAttachError {
    DuplicateRuntime { runtime_id: RuntimeId },
    ShuttingDown,
}
```

No additional executor trait is required. After reserving capacity,
`ExecutorSubmission::into_dispatch` is the admission linearization point and
the executor enqueues only the armed `ExecutorDispatch`. An unarmed submission
may be rejected; `SubmissionRejected::rollback` destroys its sink, job, and
start token inside core and returns only the rejection kind. If enqueue fails
after arming, dropping the dispatch makes one cancellation delivery attempt;
that path is admitted cancellation, not `SubmissionRejected`.

Each wrapper privately owns the sink, start token, and job. It makes exactly one
terminal delivery attempt for success, returned error, queued cancellation,
panic, or abandonment. Panic conversion is guaranteed only with
`panic = "unwind"`; `panic = "abort"` terminates the process. Dropping an
admitted dispatch or its future attempts terminal cancellation. The async
wrapper catches both future-construction and polling panic under unwind. Drive
returns classify cancellation and worker panic for executor counters; payload
success and every other returned producer failure classify as completed. The
private payload, sink, and identity are not exposed. Ordinary single destructor
panics are contained under unwind. Opaque destructors are leaked during an
already-active unwind to avoid inducing a second panic; arbitrary double-panic
inside a destructor remains process-fatal and is not promised. This
can be implemented without Tokio in `sema-core`; add workspace `futures` only
if the implementation explicitly selects and documents it.

`CompletionSender::send` is bounded and non-panicking. A terminal attempt may
report `InboxClosed`; sender-side accounting (or an explicitly deferred
reporter owned by the sender) records that failure. `Drop` cannot return a
delivery report. Task 02 defines this ownership shape; Task 03 owns registration
and rejection rollback.

`PreparedExternalOperation` has private fields and exactly three constructors:

```rust
impl PreparedExternalOperation {
    pub fn interruptible_async(/* kind, decoder, resource, async job */) -> Self;
    pub fn interruptible_blocking(/* kind, decoder, resource, blocking job */) -> Self;
    pub fn quarantined_blocking(
        /* kind, decoder, QuarantineBound, bounded blocking job */
    ) -> Self;
}
```

Constructors enforce valid `ResourceClass`/job compatibility. Producers provide
only declared completion kind, decoder, resource, and job inputs appropriate to
the selected constructor. The runtime allocates `RuntimeId` (at runtime
construction) and all operation/wait/generation identity during registration;
producers do not supply identities, sinks, or start tokens.

`sema-vm::Runtime` privately owns its `CompletionRegistrar`. The registrar
accepts only identities issued by that runtime and consumes the prepared
operation into a split binding: the VM keeps the decoder/resource/registration
half on the runtime thread, while only the opaque submission and its later
dispatch/future/envelope cross threads. No API reconstructs a registrar for an
existing `RuntimeId`, so another crate cannot inject completion authority into
an existing runtime.

`ExecutorDispatch` has only `Async` and `Blocking`; do not add parallel job or
dispatch abstractions. `RunningSubmission` may remain the admission
receipt used by `ExecutorLease::submit` if Task 03 needs it; it owns no job,
resource, or sink.

## Resource classes

Only interruptible and quarantined-bounded resources are constructible.
`QuarantineBound::hard_deadline(Duration)` is fallible and rejects zero.
`QuarantineBound::finite_work(&'static str, NonZeroU64)` cannot receive zero.
Expose a read-only descriptor and accessors for Task 05 accounting:

```rust
pub enum QuarantineBoundDescriptor {
    HardDeadline(Duration),
    FiniteWork { kind: &'static str, maximum_units: NonZeroU64 },
}

impl QuarantineBound {
    pub fn descriptor(&self) -> QuarantineBoundDescriptor;
    pub fn hard_deadline_value(&self) -> Option<Duration>;
    pub fn finite_work_value(&self) -> Option<(&'static str, NonZeroU64)>;
}
```

Keep the one-shot `CancelHook::cancel` and bounded, repeatable `reap` contract.
Pre-armed cancellation/acquisition behavior is tested in Task 05 where concrete
jobs exist. Task 02 tests constructor compatibility and wrapper terminality.

## Native protocol and ABI compatibility

```rust
pub struct CancellationView {
    requested: bool,
    reason: Option<CancelReason>,
}

impl CancellationView {
    pub fn is_requested(&self) -> bool;
    pub fn reason(&self) -> Option<&CancelReason>;
}

pub struct NativeCallContext<'a> {
    pub task_context: &'a mut TaskContext,
    pub cancellation: CancellationView,
}

pub type NativeResult = Result<NativeOutcome, SemaError>;
pub enum NativeOutcome {
    Return(Value),
    Call(NativeCall),
    Suspend(NativeSuspend),
}
```

The shell above is introduced as the first prerequisite of Task 3 so
`CompletionDecoder` compiles. `NativeCallContext` contains only `&mut TaskContext` and an owned cancellation
snapshot. It contains no concrete interpreter, output capability, evaluator callback, or generic
call capability. Native code requests Sema execution only through
`NativeOutcome::Call`; Task 03 adds runtime mechanics and Task 06 owns output
routing.

Preserve the public `NativeFn.func` ABI exactly as
`Fn(&EvalContext, &[Value]) -> Result<Value, SemaError>` and preserve all current
constructors, including `with_payload`. Add a private optional `runtime_func`,
an internal `invoke_runtime`, and public `simple_result` and
`with_context_result` constructors for runtime-aware implementations. Calling a
runtime-aware native through legacy `func` returns a clear internal error. Task
02 does not migrate call sites or reinterpret legacy constructors.

Continuation types remain consuming and traceable. `NativeCall`,
`NativeSuspend`, waits, and `ResumeInput` define requests only. Timer queues,
promise/channel lookup, actual suspension, and `NativeOutcome::Call` execution
are deferred.

## Structured cancellation and timeout conditions

Do not add public `SemaError` variants. Constructors build
`SemaError::Condition(Value)` and extend `CONDITION_TYPES` with `cancelled` and
`timeout`.

Stable cancellation map keys are `:type`, `:message`, `:reason`, and optional
`:root-id`, `:scope-id`, `:operation-id`, `:operation`, `:duration-ms`, and
`:resource-kind`. Stable timeout keys are `:type`, `:message`, `:operation`, and
`:duration-ms`, plus optional `:operation-id`. `:type` is `:cancelled` or
`:timeout`; `:reason` is a stable keyword for the `CancelReason` variant.

IDs are encoded losslessly as decimal strings because Sema numeric values cannot
represent every `u64`. Durations remain exact `u64` decimal strings for the same
reason. Rust tests assert exact maps. Language predicates are Task 04.

## Task context shell

Task 3 declares `TaskContext` with private storage so `NativeCallContext` can
name it. Task 5 implements only its typed extension behavior:

```rust
pub trait TaskLocalValue: Trace {
    fn inherit(&self) -> Rc<dyn TaskLocalValue>;
    fn as_any(&self) -> &dyn Any;
}

pub struct TaskContext {
    extensions: HashMap<TypeId, Rc<dyn TaskLocalValue>>,
}

#[derive(Clone)]
pub struct TaskContextHandle(Rc<RefCell<TaskContext>>);
```

Provide typed insert/get/remove accessors and
`TaskContext::inherit_for_child()`, which calls each extension's `inherit()`.
`EvalContext` receives `Option<TaskContextHandle>` plus read-only/set/clear or
scoped accessors, initialized in `new`, `new_with_sandbox`, and every other
constructor. Do not duplicate or migrate sandbox, module cache, current file,
call-stack, tracing, usage, context-stack, or output fields. Task 06 owns the
complete field-by-field inventory, ownership migration, inheritance table,
guards, and output routing.

## Trace contract

```rust
pub trait Trace {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool;
}
```

Tracing is structural, fallible, and exact-multiplicity: emit each directly held
strong edge once, including duplicate fields that point to the same allocation.
On a failed borrow, return `false`; the collector aborts and discards its scratch
state. A visitor may already have emitted partial sink output before returning
`false`; do not claim transactional no-output.

Runtime registries, suspended records, wait entries, task contexts, and cleanup
records remain external roots. They are never registered as collector payloads
or subtracted merely because their types implement `Trace`. `cycle.rs` delegates
only when a runtime object is an actual opaque collector payload registered via
the existing payload tracer. Opaque payloads must delegate all real Sema edges;
host-only payloads emit none.

## Legacy bridge and deletion schedule

Keep current raw `u64` callback/promise signatures. Add fallible
`LegacyRuntimeBridge` conversions: raw zero means unassigned (`None`), nonzero
maps through `TaskId::try_from_raw`. This mapping is explicitly lossy and has no
runtime provenance. Existing resume-`Value` and `IoPoll` failure strings remain
behavior-preserving until their owning task deletes them.

| Legacy surface | Task 02 action | Owner | Delete when |
|---|---|---|---|
| raw spawn/cancel task `u64` | named fallible conversion only | Task 03 | runtime task store owns checked IDs |
| scheduler/run callbacks | preserve signatures | Task 03 | interpreter runtime drives roots |
| resume `Value` encoding/failure strings | preserve behavior | Task 04 | native/language adapters migrate |
| `IoBackend`, `IoPoll`, `io_spawn`, `io_block_on` | unchanged | Task 05 | all production resource callers use executor jobs |
| EvalContext ambient fields/TLS | add handle only | Task 06 | field matrix and guards migrate ownership |
| WASM local promise host | no change | Task 07 | local host implements unified admission/wake |
| remaining compatibility exports/scanner exceptions | inventory only | Task 08 | migration audit proves no caller remains |

Every bridge inventory row names the exact symbol, callers, replacement, and
deletion task. No Task 02 bridge becomes a second scheduler or semantic owner.

## Explicit deferrals

Tasks 03–08 own: queues, timers, drive turns, wait registration, rejection
rollback, root/promise/channel retention, evaluator execution of `NativeOutcome::Call`,
output routing, migration of existing `EvalContext` fields, production
`sema-io`, the WASM local host, language predicates, and final bridge deletion.

## Task 1: Implement checked IDs and relations

- [ ] Add crate-private allocator tests for one, max, permanent exhaustion, and
  process-global `RuntimeId` uniqueness.
- [ ] Add public tests for `TaskId::try_from_raw`,
  `CompletionKind::try_from_raw`, accessors, traits, and relationship axes.
- [ ] Implement the minimum types and run `cargo test -p sema-core runtime::ids`
  plus the public `ids`/`relationships` filters.

## Task 2: Implement settlements and conditions

- [ ] Test distinct outcomes and exact cancellation/timeout condition maps,
  including lossless maximum-ID and duration strings.
- [ ] Implement `SemaError::Condition(Value)` constructors and extend
  `CONDITION_TYPES`; add no error variants or predicates.
- [ ] Run the `settlement` and `condition` test filters.

## Task 3: Implement the completion compile prerequisite, resources, and executor wrappers

- [ ] In this exact order, add the minimal `Trace` signature, declare the
  private `TaskContext` shell, add `CancellationView`/`NativeCallContext`,
  `DecodedCompletion`, and `CompletionDecoder`; then add completion envelopes, resources,
  `PreparedExternalOperation`, registrar binding, and executor wrappers.
  Do not implement tracing or native outcomes/continuations/ABI changes yet.
- [ ] Test `ExternalCompletion: Send`, while a deliberately non-`Send` decoder,
  prepared binding, and resource remain runtime-local; test typed decode
  failure, constructor compatibility, quarantine descriptors, attachment
  errors, capability-safe registration, and internal rejection destruction.
- [ ] Test each dispatch terminal path: return, error, queued cancellation,
  construction panic, poll/run panic (under unwind), dispatch drop, and future
  drop. Assert one bounded terminal delivery attempt and no nameable sink.
- [ ] Test reserve -> arm (`into_dispatch`) -> enqueue ordering: rejected work
  remains unarmed; admitted queues contain only dispatches; post-arm enqueue
  failure attempts cancellation and is not `SubmissionRejected`. Test closed
  inbox accounting in the sender/reporter rather than as a `Drop` return.
- [ ] Implement under `runtime::{completion,resource,executor}` without touching
  `io_backend.rs`; run the `completion`, `resource`, and `executor` filters.

## Task 4: Complete native protocol, dual path, and tracing

- [ ] Test return/call/suspend requests, each `ResumeInput`, consuming
  continuation behavior, legacy constructor compatibility, runtime-aware legacy
  invocation error, and `with_payload` preservation.
- [ ] Test exact duplicate-edge multiplicity and a failed borrow after partial
  sink emission. Test payload delegation only for an actual opaque payload.
- [ ] Implement and run the `native` filter plus `cargo test -p sema-core cycle`.

## Task 5: Implement the task-context extension shell

- [ ] Test typed insert/get/remove and child inheritance with two extension
  types; do not write a named EvalContext-field migration table here.
- [ ] Add `TaskContextHandle` to every `EvalContext` constructor and test absent,
  installed, cloned-handle, and inherited-child behavior.
- [ ] Run the `task_context` filter and `cargo test -p sema-core context`.

## Task 6: Add legacy conversions and source guards

- [ ] Add named fallible bridge conversions while preserving raw callback and
  promise signatures and existing failure text.
- [ ] Update inventory rows and inspect intentional baseline changes with
  `scripts/check-unified-runtime-legacy.sh`.
- [ ] Run `vm_async_test` and `runtime_conformance_test`; record unchanged RED
  characterization cases exactly rather than claiming them GREEN.

## Task 7: Verify, evidence, review, and commit

- [ ] Run:

```bash
cargo test -p sema-core
cargo test -p sema-lang --test runtime_conformance_test
cargo fmt --all -- --check
cargo clippy -p sema-core --all-targets -- -D warnings
jake docs-check
git diff --check
```

- [ ] Record commands, statuses, baseline changes, and remaining RED tests in
  Task 02 evidence. Obtain independent review with stable `UR-T02-R###` IDs.
- [ ] Fix each valid finding test-first and rerun affected and full gates.
- [ ] Commit only the accepted implementation/evidence with
  `refactor(runtime): define core runtime contracts`.

## Completion criteria

- Checked IDs cannot wrap; public raw construction is limited and fallible.
- Conditions use exact `SemaError::Condition` maps and preserve all `u64`s.
- Executor wrappers own private completion authority and make exactly one
  terminal delivery attempt on every admitted path, including unwind panic and
  abandonment; abort panic terminates the process.
- Prepared operations cannot pair an incompatible resource and job.
- Native runtime context has only task context and cancellation snapshot; legacy
  ABI and constructors remain source-compatible.
- Task 02 adds only the typed task-context shell and optional EvalContext handle.
- Trace is fallible, exact-multiplicity, and does not turn registries into
  collector-internal edges.
- Legacy behavior has not switched; every bridge has a staged deletion owner.
- All deferrals remain unimplemented and evidence/review are committed.
