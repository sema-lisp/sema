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
  `docs/plans/archive/2026-07-02-core2-gc.md` before editing.
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
- `crates/sema-core/src/runtime/trace.rs` — object-safe CORE-2 edge tracing for
  runtime-owned trait objects and composites.
- `crates/sema-core/src/runtime/resource.rs` — constructible resource policies.
- `crates/sema-core/tests/runtime_types_test.rs` — public contract tests.
- `docs/plans/evidence/unified-cooperative-runtime/task-02.md` — verification
  transcript and remaining RED characterization list.
- `docs/plans/reviews/unified-cooperative-runtime/task-02.md` — independent
  review findings and disposition.

**Modify**

- `crates/sema-core/src/lib.rs` — export `runtime`.
- `crates/sema-core/src/io_backend.rs` — add the abstract executor lease,
  completion sender/sink, one-shot job, and owning rejection seam used by Task
  03 fakes and the Task 05 production implementation; retain legacy methods only
  as inventoried adapters until Task 05 deletes them.
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
pub trait Trace {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool;
}
```

`Trace` is object-safe and mirrors CORE-2's `OpaqueTraceFn`/`PayloadTracer`
contract. Implementations emit each directly held strong edge exactly once;
two fields holding the same allocation emit two edges because trial-deletion
arithmetic requires multiplicity. Boxed trait objects and composites delegate
recursively. If any required `RefCell` borrow is unavailable, return `false`
and abort that collection pass cleanly; never skip an edge or partially claim a
successful trace. `cycle.rs` delegates to this trait only when such an object is
embedded in an actual CORE-2 opaque heap
node through the existing `register_payload_tracer` seam. Runtime-owned
suspended state, wait-registry entries, cleanup hooks, and task contexts are
external roots: their strong counts must remain unsubtracted, or CORE-2 could
over-subtract and collect live data. The next safe-point collection may retry
after a borrow conflict ends.

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

pub struct IdCounter<I> { /* private next value and marker */ }
impl<I> IdCounter<I> {
    pub fn allocate(&mut self) -> Result<I, IdExhausted>;
}
```

All identity types implement `Copy`, `Clone`, `Debug`, `Eq`, `Ord`, and `Hash`.
Construction is crate-controlled. Allocation starts at one, uses checked
increment, never wraps, and permanently reports `IdExhausted` after exhaustion.
Language-visible root, promise, and channel handles carry the composite identity
and reject cross-runtime use before lookup. A promise settlement is retained
while any language handle or registered observer exists, then reaped after final
handle/observer release. Channel state is retained while any endpoint handle,
buffered value, or waiter exists and is reaped after close plus final release.
Root records retain settlement while the explicit root-handle lease count is
nonzero or descendants/debug/tracing references remain; cloning a `RootHandle`
increments that count and each drop decrements it, making final-handle drop an
observable cleanup event rather than relying on `Weak` liveness.

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

pub enum DecodedCompletion {
    Returned(Value),
    Failed(SemaError),
}

pub trait CompletionDecoder: Trace {
    fn decode(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion;
}

pub struct NativeCallContext<'a> {
    pub vm: &'a mut VM,
    pub task_context: &'a mut TaskContext,
    pub output: &'a dyn OutputSink,
    pub cancellation: &'a CancellationView,
}
```

`NativeCallContext` is assembled only after runtime bookkeeping has been
extracted and every `RuntimeState` `RefCell` borrow has been dropped. Native
functions, decoders, continuations, output hooks, and cancellation callbacks
therefore run with no outstanding runtime-state borrow; Task 03 applies their
returned transition only after reacquiring state.

The envelope cannot contain `Value`, `Env`, `SemaError`, `Rc`, or a VM
continuation. `ExternalFailure` is a send-safe code/message/source structure,
not an erased `SemaError`. `CompletionKind` is a crate-controlled, non-zero,
send-safe discriminator. The wait registry stores the expected kind and rejects
a wrong-kind completion before invoking its decoder. A payload that has the
correct declared kind but fails the decoder's concrete downcast becomes a named
`Decode` failure for that operation; wrong-kind delivery never changes the
waiting task's outcome.

`CompletionDecoder: Trace` is mandatory because a decoder may remain in the
wait registry across CORE-2 collection and may retain Sema values needed to
decode/resume. Its `Trace` implementation visits every such value. A decoder
must not hide `Value`, `Env`, or another traceable object in an opaque host
closure; required roots live in the decoder or continuation and are traced. A
decoder cannot suspend or bypass the continuation: it consumes the raw worker
result into exactly one `DecodedCompletion`, which the runtime converts to
`ResumeInput::Returned`/`Failed` before consuming the continuation.

The interpreter runtime selects `CompletionKind` while registering the wait and
copies it into the private `CompletionSink` defined in Task 05. The executor,
not the worker job, owns that sink. A worker job can only return its send-safe
result; it cannot omit or duplicate delivery, select a kind, forge identity
fields, or return a runtime-side decoder/resource hook. The executor converts a
job panic to `ExternalFailureCode::WorkerPanic` and owns one terminal sink
delivery for every admitted job.

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
    External(Box<PreparedExternalOperation>),
}

pub enum ChannelWait {
    Send { channel: ChannelId, value: Value },
    Receive { channel: ChannelId },
}

pub enum ResumeInput {
    Returned(Value),
    Failed(SemaError),
    Cancelled(CancelReason),
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
every stored `Value`. Implement `Trace` for `NativeSuspend`, `WaitRequest`,
`WaitKind`, `PreparedExternalOperation`, and `ResourceClass`. The chain visits
the continuation, decoder, and `CancelHook: Trace`; a host-only hook implements
an empty trace. `PreparedExternalOperation` ignores only the send-only `ExecutorJob`
and atomic queue token, which cannot contain runtime-thread state. Opaque host
closures may not hide a `Value`, `Env`, or traceable object. Required Sema roots
live in the traced continuation, decoder, or hook.

```rust
pub enum ResourceClassDescriptor {
    Interruptible,
    QuarantinedBounded { bound: QuarantineBound },
}

pub enum ResourceClass {
    Interruptible { cancel: Box<dyn CancelHook> },
    QuarantinedBounded { bound: QuarantineBound },
}

#[derive(Clone, Copy)]
pub struct QuarantineBound {
    kind: QuarantineBoundKind,
}

#[derive(Clone, Copy)]
enum QuarantineBoundKind {
    HardDeadline(Duration),
    FiniteWork { kind: &'static str, maximum_units: NonZeroU64 },
}

pub enum QuarantineBoundError {
    ZeroDeadline,
}

impl QuarantineBound {
    pub fn hard_deadline(duration: Duration) -> Result<Self, QuarantineBoundError> {
        if duration.is_zero() {
            return Err(QuarantineBoundError::ZeroDeadline);
        }
        Ok(Self {
            kind: QuarantineBoundKind::HardDeadline(duration),
        })
    }

    pub fn finite_work(kind: &'static str, maximum_units: NonZeroU64) -> Self {
        Self {
            kind: QuarantineBoundKind::FiniteWork {
                kind,
                maximum_units,
            },
        }
    }
}

pub trait CancelHook: Trace {
    /// Called exactly once for the cancellation request.
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError>;

    /// Bounded/nonblocking recovery; called only by `CleanupRegistry` after
    /// `PendingReap` or `CancelHookError`, and may be polled repeatedly.
    fn reap(&mut self) -> Result<ReapDisposition, CancelHookError>;
}

pub enum CancelDisposition {
    Reaped,
    PendingReap,
}

pub enum ReapDisposition {
    Reaped,
    Pending,
}

pub enum CancelHookErrorCode {
    AbortFailed,
    CloseFailed,
    KillFailed,
    WakeFailed,
    ReapFailed,
}

pub struct CancelHookError {
    pub code: CancelHookErrorCode,
    pub operation_id: OperationId,
    pub resource_kind: &'static str,
    pub resource_id: String,
    pub message: String,
    pub source: Option<String>,
}

const DISPATCH_QUEUED: u8 = 0;
const DISPATCH_RUNNING: u8 = 1;
const DISPATCH_CANCELLED: u8 = 2;

pub struct ExecutorJobControl;

pub struct ExecutorCancelHandle {
    state: Arc<AtomicU8>,
}

/// Non-cloneable proof consumed by the executor's dequeue decision.
pub struct ExecutorStartToken {
    state: Arc<AtomicU8>,
}

pub enum CancelBeforeStart {
    CancelledQueued,
    AlreadyCancelled,
    AlreadyRunning,
}

pub enum ExecutorStartDecision {
    Run,
    CompleteCancelled,
}

impl ExecutorJobControl {
    pub fn new() -> (ExecutorCancelHandle, ExecutorStartToken) {
        let state = Arc::new(AtomicU8::new(DISPATCH_QUEUED));
        (
            ExecutorCancelHandle {
                state: Arc::clone(&state),
            },
            ExecutorStartToken { state },
        )
    }
}

impl ExecutorCancelHandle {
    pub fn cancel_before_start(&self) -> CancelBeforeStart {
        match self.state.compare_exchange(
            DISPATCH_QUEUED,
            DISPATCH_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => CancelBeforeStart::CancelledQueued,
            Err(DISPATCH_CANCELLED) => CancelBeforeStart::AlreadyCancelled,
            Err(DISPATCH_RUNNING) => CancelBeforeStart::AlreadyRunning,
            Err(state) => unreachable!("invalid executor job state {state}"),
        }
    }
}

impl ExecutorStartToken {
    pub fn claim_for_run(self) -> ExecutorStartDecision {
        match self.state.compare_exchange(
            DISPATCH_QUEUED,
            DISPATCH_RUNNING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => ExecutorStartDecision::Run,
            Err(DISPATCH_CANCELLED) => ExecutorStartDecision::CompleteCancelled,
            Err(state) => unreachable!("invalid executor job start state {state}"),
        }
    }
}

/// cfg-neutral admission boundary. `send` enqueues and coalescingly requests a
/// host drive turn when the runtime is not already scheduled.
pub trait CompletionSender: Send + Sync + 'static {
    fn send(&self, completion: ExternalCompletion) -> CompletionDelivery;
}

pub enum CompletionDelivery {
    Enqueued,
    InboxClosed,
}

// crates/sema-core/src/runtime/completion.rs
pub(in crate::runtime) struct CompletionSink {
    sender: Arc<dyn CompletionSender>,
    runtime_id: RuntimeId,
    wait_id: WaitId,
    generation: WaitGeneration,
    operation_id: OperationId,
    kind: CompletionKind,
}

impl CompletionSink {
    pub(in crate::runtime) fn for_registered_wait(
        sender: Arc<dyn CompletionSender>,
        runtime_id: RuntimeId,
        wait_id: WaitId,
        generation: WaitGeneration,
        operation_id: OperationId,
        kind: CompletionKind,
    ) -> Self {
        Self {
            sender,
            runtime_id,
            wait_id,
            generation,
            operation_id,
            kind,
        }
    }

    /// Consumes the only delivery capability for this admitted job.
    pub(in crate::runtime) fn complete(
        self,
        result: Result<SendPayload, ExternalFailure>,
    ) -> CompletionDelivery {
        let Self {
            sender,
            runtime_id,
            wait_id,
            generation,
            operation_id,
            kind,
        } = self;
        sender.send(ExternalCompletion {
            runtime_id,
            wait_id,
            generation,
            operation_id,
            kind,
            result,
        })
    }
}

/// Opaque, owning queue item. Its fields are private and it is not `Clone`.
pub struct ExecutorSubmission {
    job: ExecutorJob,
    sink: CompletionSink,
    start_token: ExecutorStartToken,
}

impl ExecutorSubmission {
    pub fn for_registered_wait(
        sender: Arc<dyn CompletionSender>,
        runtime_id: RuntimeId,
        wait_id: WaitId,
        generation: WaitGeneration,
        operation_id: OperationId,
        kind: CompletionKind,
        job: ExecutorJob,
        start_token: ExecutorStartToken,
    ) -> Self {
        /* validate identities, then call private CompletionSink constructor */
    }
}

pub enum ExecutorDriveReport {
    Delivered(CompletionDelivery),
}

mod executor_submission_sealed {
    pub trait Sealed {}
}

/// The only cross-crate executor capability. `sema-io` may invoke these
/// methods but cannot implement the trait or obtain the private sink.
pub trait ExecutorSubmissionDriver: executor_submission_sealed::Sealed + Send {
    fn operation_id(&self) -> OperationId;
    fn drive(self: Box<Self>) -> ExecutorDriveReport;
}

impl executor_submission_sealed::Sealed for ExecutorSubmission {}

impl ExecutorSubmissionDriver for ExecutorSubmission {
    fn operation_id(&self) -> OperationId { /* private-field projection */ }

    fn drive(self: Box<Self>) -> ExecutorDriveReport {
        /* claim start token; run/catch panic or cancel; consume sink once */
    }
}

pub type AsyncJobFuture = Pin<
    Box<dyn Future<Output = Result<SendPayload, ExternalFailure>> + Send + 'static>,
>;

pub trait InterruptibleAsyncJob: Send + 'static {
    fn run(self: Box<Self>) -> AsyncJobFuture;
}

pub trait BlockingBoundedJob: Send + 'static {
    fn run(self: Box<Self>) -> Result<SendPayload, ExternalFailure>;
}

pub trait InterruptibleBlockingJob: Send + 'static {
    fn run(self: Box<Self>) -> Result<SendPayload, ExternalFailure>;
}

pub enum ExecutorJob {
    InterruptibleAsync(Box<dyn InterruptibleAsyncJob>),
    InterruptibleBlocking(Box<dyn InterruptibleBlockingJob>),
    QuarantinedBlocking {
        bound: QuarantineBound,
        job: Box<dyn BlockingBoundedJob>,
    },
}

pub struct RunningSubmission {
    pub operation_id: OperationId,
}

pub enum SubmitErrorKind {
    LeaseShuttingDown,
    QueueClosed,
    AdmissionRejected,
    HostStartRejected,
}

pub struct SubmissionRejected {
    kind: SubmitErrorKind,
    submission: ExecutorSubmission,
}

pub struct RejectedSubmissionRollback {
    pub kind: SubmitErrorKind,
    pub job: ExecutorJob,
    pub start_token: ExecutorStartToken,
}

impl SubmissionRejected {
    /// Consumes and destroys the terminal sink before returning rollback owners.
    pub fn into_rollback(self) -> RejectedSubmissionRollback {
        /* destructure privately, drop sink, return kind/job/start token */
    }
}

/// The sole unregistered external-wait request.
pub struct PreparedExternalOperation {
    pub completion_kind: CompletionKind,
    pub decoder: Box<dyn CompletionDecoder>,
    pub resource: ResourceClass,
    pub cancel: ExecutorCancelHandle,
    pub job: ExecutorJob,
    pub start_token: ExecutorStartToken,
}

pub struct ExecutorSnapshot {
    pub queued: usize,
    pub running_interruptible: usize,
    pub running_quarantined: usize,
    pub completed: u64,
    pub cancelled: u64,
    pub panicked: u64,
    pub undeliverable: u64,
}

pub struct ExecutorShutdown {
    pub snapshot: ExecutorSnapshot,
    pub deadline_exceeded: bool,
}

pub trait ExecutorLease: Send + Sync {
    fn submit(
        &self,
        submission: ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected>;
    fn snapshot(&self) -> ExecutorSnapshot;
    fn shutdown(&self, deadline: Instant) -> ExecutorShutdown;
}

pub trait IoExecutor: Send + Sync {
    fn attach_runtime(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Arc<dyn ExecutorLease>, ExecutorAttachError>;
    fn snapshot(&self) -> ExecutorSnapshot;
}
```

`CompletionSink` belongs to `sema-core::runtime::completion`; both its
constructor and consuming `complete` method are `pub(in crate::runtime)`. Rust
privacy cannot privilege the separate `sema-io` crate, so `sema-io` never names
or receives a sink. Runtime registration calls the public, checked
`ExecutorSubmission::for_registered_wait` factory, which delegates sink
construction to `completion.rs` after validating the registered identities and
bundles the sink with the job and start token. The resulting queue item has
private fields and no extraction or clone API.

`ExecutorSubmissionDriver` is public only so the separate `sema-io` crate can
dequeue and invoke it; its private supertrait seals implementation to
`sema-core`. The core implementation claims the start token, invokes the job,
catches panic, maps queued cancellation/panic/return, and consumes the private
sink exactly once. Worker jobs receive no submission or delivery capability.
The lease has exclusive custody after admission. On rejection it returns an
opaque `SubmissionRejected`; `into_rollback` destroys the sink inside
`sema-core` and returns only the job, start token, and rejection kind needed to
undo registration. Rejected callers therefore retain owning rollback without a
sink or executable terminal capability they could use to forge delivery.

`attach_runtime` rejects duplicate `RuntimeId`s and shutdown pools. Runtime
construction therefore returns `Result<Runtime, ExecutorAttachError>`. Lease
shutdown first rejects submissions, then cancels/drains that runtime's jobs,
then unregisters the `RuntimeId`; only final process-executor shutdown may stop
shared workers. Browser/local-host Promise work does not implement
`ExecutorJob` and never enters this `Send` pool. Task 07 owns its concrete
single-threaded execution while using the same completion-admission/wake
boundary.

Construction rejects a zero deadline or zero work bound. A finite-work producer
must prove its unit count before dispatch and must not expand the bound while it
runs. `CancelHook::cancel` is one-shot; `reap` is bounded/nonblocking and
idempotent under repeated cleanup polling. Tests distinguish their call counts.
The resource class has no third variant.

`PreparedExternalOperation::new` calls `ExecutorJobControl::new` before wait
registration. `WaitKind::External(Box<PreparedExternalOperation>)` is the sole
unregistered request; there is no second `ExternalWait` that could duplicate or
double-move its fields. When Task 03 applies `NativeSuspend`, the runtime
destructures the box exactly once, stores the runtime-side
`ExecutorCancelHandle` in its distinct `RegisteredExternalWait`, and Task 05
submits the non-cloneable executor-side token with the job. Cancellation first
calls `cancel_before_start`, then detaches the wait-owned resource association
exactly once regardless of which CAS result it observed. For an
`Interruptible` operation, that path always invokes `CancelHook::cancel` exactly
once: the hook records sticky cancellation for a not-yet-created handle and
also closes/releases any existing child, stream, or resource named by the
prepared operation. `Reaped` removes the entry; `PendingReap`/error atomically
transfers that same owned entry into cleanup-retry state. Repeated cancellation
finds no wait-attached cancellable entry and does not call `cancel` again, even
though cleanup may remain live. If the CAS returns `CancelledQueued`, the
executor later observes `CompleteCancelled`, drops the job without running it,
and owns the one terminal completion. If dequeue wins and returns `Run`, the
same hook reaches the acquisition/running operation. The atomic transition
governs only whether the job body runs and which executor path consumes the
sink; it never suppresses resource cleanup. No private executor job ID is
needed.

`CancelHookError` is runtime-safe and contains no `Value`, `Env`, `SemaError`,
`Rc`, or VM state. An underlying already-gone/not-found result maps to successful
`CancelDisposition::Reaped` or `ReapDisposition::Reaped` and MUST NOT be returned
as `CancelHookError`. `cancel` is the one-shot request: `Reaped` removes the
resource entry, while `PendingReap` or a real error leaves sticky cancellation
set and transfers/retains the still-owned resource/hook in `CleanupRegistry`.
The runtime records a deduplicated suppressed cleanup diagnostic and does not
decrement live-resource accounting. Neither repeated user cancellation nor the
registry invokes `cancel` again; bounded cleanup turns call only `reap`, which
may return `Pending` or a new retained error until it succeeds. Explicit
shutdown is non-clean while the entry remains: later `Reaped` removes it;
deadline expiry reports its operation/resource identity and last error as an
invariant failure.

`AlreadyRunning` does not imply that the worker has acquired an OS resource yet.
Every interruptible prepared operation MUST therefore create a pre-armed,
sticky cancellation hook/token pair before submission. The runtime-side
`CancelHook::cancel` records cancellation in the shared state even when no abort
handle is attached. The job-side token MUST make acquisition itself
interruptible: either the acquisition future/syscall selects or polls against a
cancellation token plus wake primitive from that pre-submission state, or the
concrete abort handle is installed before the resource's first potentially
blocking poll. A check immediately before a potentially unbounded acquisition
is insufficient because cancellation can arrive while acquisition is blocked.

Post-acquisition attachment is allowed only when resource construction is
nonblocking and attachment completes through the synchronized shared state
before the resource's first potentially blocking use. If cancellation won
before construction, the job skips construction and returns `Cancelled`; if it
races with construction/attachment, attachment invokes the idempotent abort
immediately. If attachment wins, a later cancel invokes the attached abort. An
API whose cancel handle appears only after a potentially unbounded acquisition,
and whose acquisition cannot select/poll a cancellation wake, is not
`Interruptible`: model that acquisition as a separate interruptible wait, prove
a `QuarantinedBounded` deadline/work bound, or classify the operation
`PROHIBITED`. A hook that merely looks up a not-yet-created handle, or a token
that only checks before entering a blocking acquisition, is invalid.

A `QuarantinedBounded` job MUST have only an immutable, owned `Send` input
snapshot before `claim_for_run` returns `Run`. Preparation and queue residency
must not acquire a resource, start work, mutate external state, or create cleanup
that dropping the queued job would strand. If an operation needs any such
pre-run effect, classify it `Interruptible` with an exactly-once hook that
releases the effect, or `PROHIBITED`; it is not a valid quarantined job.

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
`ExecutorJob` and an executor-private completion sink cross to the pool, and
that the job never receives that sink. Add deterministic control-pair tests for
`Queued -> Cancelled`, `Queued -> Running`, repeated cancel, and the impossible
second start claim. With a fake pre-armed interruptible hook/token, cancel once
before acquisition, once while a barrier has paused inside acquisition, and
once between nonblocking construction and abort attachment. Prove respectively
that acquisition is skipped, the cancellation wake unblocks acquisition, and
attachment aborts exactly once before first blocking use. The queued case must
also prove one hook call, no second call after repeated cancellation, and release
of an existing fake resource. A quarantined fixture must prove preparation and
queue residency contain only immutable input and perform no acquisition,
mutation, or work before `Run`.

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
test proving stored values are visited by tracing. Add nested boxed-trait and
composite tests that preserve duplicate-edge multiplicity, plus a deliberately
held `RefCell` borrow proving `trace` returns `false` and the collection pass
aborts without treating a partial edge set as complete. Exercise
`NativeSuspend -> WaitRequest -> PreparedExternalOperation -> decoder/resource
hook` traversal as one object graph.
Add a lifetime regression where a cycle strongly retained by a suspended
continuation/decoder/context survives collection as an external root; after the
runtime owner removes/drops it, the next safe point collects it. This guards
against incorrectly registering runtime roots as internal CORE-2 edges.

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
