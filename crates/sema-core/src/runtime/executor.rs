use std::cell::RefCell;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use crate::cycle::GcEdge;

use super::{
    CompletionDecoder, CompletionDelivery, CompletionKind, CompletionSender, ExternalCompletion,
    ExternalFailure, ExternalFailureCode, IdCounter, IdExhausted, InterruptibleResource,
    OperationId, QuarantineBound, ResourceClass, RuntimeId, SendPayload, Trace, WaitGeneration,
    WaitId,
};

type JobResult = Result<SendPayload, ExternalFailure>;
type AsyncJobFuture = Pin<Box<dyn Future<Output = JobResult> + Send + 'static>>;
type AsyncJob = Box<dyn FnOnce() -> AsyncJobFuture + Send + 'static>;
type BlockingJob = Box<dyn FnOnce() -> JobResult + Send + 'static>;

enum PreparedJob {
    Async(AsyncJob),
    Blocking {
        class: BlockingDispatchClass,
        job: BlockingJob,
    },
}

pub struct PreparedExternalOperation {
    kind: CompletionKind,
    decoder: Box<dyn CompletionDecoder>,
    resource: ResourceClass,
    job: PreparedJob,
}

impl PreparedExternalOperation {
    #[doc(hidden)]
    pub fn completion_kind(&self) -> CompletionKind {
        self.kind
    }
    pub fn interruptible_async<F, Fut>(
        kind: CompletionKind,
        decoder: Box<dyn CompletionDecoder>,
        resource: InterruptibleResource,
        job: F,
    ) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = JobResult> + Send + 'static,
    {
        let (resource_kind, hook) = resource.into_parts();
        Self {
            kind,
            decoder,
            resource: ResourceClass::interruptible(resource_kind, hook),
            job: PreparedJob::Async(Box::new(move || Box::pin(job()))),
        }
    }

    pub fn interruptible_blocking<F>(
        kind: CompletionKind,
        decoder: Box<dyn CompletionDecoder>,
        resource: InterruptibleResource,
        job: F,
    ) -> Self
    where
        F: FnOnce() -> JobResult + Send + 'static,
    {
        let (resource_kind, hook) = resource.into_parts();
        Self {
            kind,
            decoder,
            resource: ResourceClass::interruptible(resource_kind, hook),
            job: PreparedJob::Blocking {
                class: BlockingDispatchClass::Interruptible,
                job: Box::new(job),
            },
        }
    }

    pub fn quarantined_blocking<F>(
        kind: CompletionKind,
        decoder: Box<dyn CompletionDecoder>,
        bound: QuarantineBound,
        job: F,
    ) -> Self
    where
        F: FnOnce() -> JobResult + Send + 'static,
    {
        Self {
            kind,
            decoder,
            resource: ResourceClass::quarantined(bound),
            job: PreparedJob::Blocking {
                class: BlockingDispatchClass::QuarantinedBounded,
                job: Box::new(job),
            },
        }
    }
}

impl Trace for PreparedExternalOperation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        self.decoder.trace(sink) && self.resource.trace(sink)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct RuntimeIssuedCompletionIdentity {
    runtime_id: RuntimeId,
    wait_id: WaitId,
    generation: WaitGeneration,
    operation_id: OperationId,
    kind: CompletionKind,
    authority: u64,
}

impl RuntimeIssuedCompletionIdentity {
    pub fn runtime_id(&self) -> RuntimeId {
        self.runtime_id
    }

    pub fn wait_id(&self) -> WaitId {
        self.wait_id
    }

    pub fn generation(&self) -> WaitGeneration {
        self.generation
    }

    pub fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    pub fn kind(&self) -> CompletionKind {
        self.kind
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum BindCompletionError {
    #[error("completion identity was issued by a different registrar")]
    ForeignIdentity,
    #[error(
        "completion kind mismatch: identity expects {expected:?}, operation declares {declared:?}"
    )]
    KindMismatch {
        expected: CompletionKind,
        declared: CompletionKind,
    },
}

impl BindCompletionError {
    pub fn expected(&self) -> CompletionKind {
        match self {
            Self::KindMismatch { expected, .. } => *expected,
            Self::ForeignIdentity => panic!("foreign identity has no expected kind"),
        }
    }
    pub fn declared(&self) -> CompletionKind {
        match self {
            Self::KindMismatch { declared, .. } => *declared,
            Self::ForeignIdentity => panic!("foreign identity has no declared kind"),
        }
    }
}

#[derive(Clone, Copy)]
struct CompletionIdentity {
    runtime_id: RuntimeId,
    wait_id: WaitId,
    generation: WaitGeneration,
    operation_id: OperationId,
    kind: CompletionKind,
}

#[doc(hidden)]
pub struct CompletionRegistrar {
    runtime_id: RuntimeId,
    authority: u64,
    sender: Arc<dyn CompletionSender>,
    waits: RefCell<IdCounter<WaitId>>,
    generations: RefCell<IdCounter<WaitGeneration>>,
    operations: RefCell<IdCounter<OperationId>>,
}

impl CompletionRegistrar {
    #[doc(hidden)]
    pub fn register(sender: Arc<dyn CompletionSender>) -> Result<(RuntimeId, Self), IdExhausted> {
        let runtime_id = RuntimeId::allocate()?;
        Ok((
            runtime_id,
            Self {
                runtime_id,
                authority: runtime_id.get(),
                sender,
                waits: RefCell::new(IdCounter::new()),
                generations: RefCell::new(IdCounter::new()),
                operations: RefCell::new(IdCounter::new()),
            },
        ))
    }

    #[doc(hidden)]
    pub fn issue_identity(
        &self,
        kind: CompletionKind,
    ) -> Result<RuntimeIssuedCompletionIdentity, IdExhausted> {
        Ok(RuntimeIssuedCompletionIdentity {
            runtime_id: self.runtime_id,
            wait_id: self.waits.borrow_mut().allocate()?,
            generation: self.generations.borrow_mut().allocate()?,
            operation_id: self.operations.borrow_mut().allocate()?,
            kind,
            authority: self.authority,
        })
    }

    #[doc(hidden)]
    pub fn bind(
        &self,
        identity: RuntimeIssuedCompletionIdentity,
        prepared: PreparedExternalOperation,
    ) -> Result<ExternalOperationBinding, BindCompletionError> {
        if identity.runtime_id != self.runtime_id || identity.authority != self.authority {
            destroy_prepared(prepared);
            return Err(BindCompletionError::ForeignIdentity);
        }
        if identity.kind != prepared.kind {
            let error = BindCompletionError::KindMismatch {
                expected: identity.kind,
                declared: prepared.kind,
            };
            destroy_prepared(prepared);
            return Err(error);
        }
        let identity = CompletionIdentity {
            runtime_id: identity.runtime_id,
            wait_id: identity.wait_id,
            generation: identity.generation,
            operation_id: identity.operation_id,
            kind: identity.kind,
        };
        let PreparedExternalOperation {
            kind: _declared_kind,
            decoder,
            resource,
            job,
        } = prepared;
        let control = Arc::new(AtomicU8::new(QUEUED));
        let sink = CompletionSink {
            sender: Arc::clone(&self.sender),
            identity,
        };
        Ok(ExternalOperationBinding {
            runtime: RuntimeOperationBinding {
                decoder,
                resource,
                queue_cancel: ExecutorCancelHandle {
                    state: Arc::clone(&control),
                },
            },
            submission: ExecutorSubmission {
                identity,
                sink: Some(sink),
                start: ExecutorStartToken { state: control },
                job: Some(job),
            },
        })
    }
}

pub struct ExternalOperationBinding {
    runtime: RuntimeOperationBinding,
    submission: ExecutorSubmission,
}

impl ExternalOperationBinding {
    #[doc(hidden)]
    pub fn split(self) -> (RuntimeOperationBinding, ExecutorSubmission) {
        (self.runtime, self.submission)
    }
}

pub struct RuntimeOperationBinding {
    decoder: Box<dyn CompletionDecoder>,
    resource: ResourceClass,
    queue_cancel: ExecutorCancelHandle,
}

impl RuntimeOperationBinding {
    #[doc(hidden)]
    pub fn into_parts(
        self,
    ) -> (
        Box<dyn CompletionDecoder>,
        ResourceClass,
        ExecutorCancelHandle,
    ) {
        (self.decoder, self.resource, self.queue_cancel)
    }
}

const QUEUED: u8 = 0;
const CANCELLED: u8 = 1;
const RUNNING: u8 = 2;

pub struct ExecutorCancelHandle {
    state: Arc<AtomicU8>,
}

impl ExecutorCancelHandle {
    pub fn cancel_before_start(&self) -> CancelBeforeStart {
        match self
            .state
            .compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => CancelBeforeStart::CancelledQueued,
            Err(CANCELLED) => CancelBeforeStart::AlreadyCancelled,
            Err(RUNNING) => CancelBeforeStart::AlreadyRunning,
            Err(_) => unreachable!("queue control has a valid state"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelBeforeStart {
    CancelledQueued,
    AlreadyCancelled,
    AlreadyRunning,
}

struct ExecutorStartToken {
    state: Arc<AtomicU8>,
}

impl ExecutorStartToken {
    fn claim_for_run(&self) -> ExecutorStartDecision {
        match self
            .state
            .compare_exchange(QUEUED, RUNNING, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(RUNNING) => ExecutorStartDecision::Run,
            Err(CANCELLED) => ExecutorStartDecision::CompleteCancelled,
            Err(_) => unreachable!("queue control has a valid state"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutorStartDecision {
    Run,
    CompleteCancelled,
}

struct CompletionSink {
    sender: Arc<dyn CompletionSender>,
    identity: CompletionIdentity,
}

impl CompletionSink {
    fn deliver(self, result: JobResult) -> CompletionDelivery {
        self.sender.send(ExternalCompletion {
            runtime_id: self.identity.runtime_id,
            wait_id: self.identity.wait_id,
            generation: self.identity.generation,
            operation_id: self.identity.operation_id,
            kind: self.identity.kind,
            result,
        })
    }
}

pub struct ExecutorSubmission {
    identity: CompletionIdentity,
    sink: Option<CompletionSink>,
    start: ExecutorStartToken,
    job: Option<PreparedJob>,
}

impl ExecutorSubmission {
    pub fn operation_id(&self) -> OperationId {
        self.identity.operation_id
    }

    pub fn into_dispatch(mut self) -> ExecutorDispatch {
        let sink = self.sink.take().expect("submission owns its sink");
        let job = self.job.take().expect("submission owns its job");
        let identity = self.identity;
        match job {
            PreparedJob::Async(job) => ExecutorDispatch::Async(AsyncExecutorDispatch {
                identity,
                sink: Some(sink),
                start: Some(self.start),
                job: Some(job),
            }),
            PreparedJob::Blocking { class, job } => {
                ExecutorDispatch::Blocking(BlockingExecutorDispatch {
                    identity,
                    class,
                    sink: Some(sink),
                    start: Some(self.start),
                    job: Some(job),
                })
            }
        }
    }

    pub fn reject(self, kind: SubmitErrorKind) -> SubmissionRejected {
        SubmissionRejected {
            kind,
            submission: Box::new(self),
        }
    }
}

pub enum ExecutorDispatch {
    Async(AsyncExecutorDispatch),
    Blocking(BlockingExecutorDispatch),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockingDispatchClass {
    Interruptible,
    QuarantinedBounded,
}

/// Terminal executor accounting, intentionally independent of payload details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutorTerminal {
    Completed,
    Cancelled,
    WorkerPanic,
}

/// The terminal class and whether its private completion reached the runtime inbox.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutorDriveReport {
    pub terminal: ExecutorTerminal,
    pub delivery: CompletionDelivery,
}

fn terminal_for(result: &JobResult) -> ExecutorTerminal {
    match result {
        Err(failure) if failure.code() == ExternalFailureCode::Cancelled => {
            ExecutorTerminal::Cancelled
        }
        Err(failure) if failure.code() == ExternalFailureCode::WorkerPanic => {
            ExecutorTerminal::WorkerPanic
        }
        Ok(_) | Err(_) => ExecutorTerminal::Completed,
    }
}

pub struct BlockingExecutorDispatch {
    identity: CompletionIdentity,
    class: BlockingDispatchClass,
    sink: Option<CompletionSink>,
    start: Option<ExecutorStartToken>,
    job: Option<BlockingJob>,
}

impl BlockingExecutorDispatch {
    pub fn operation_id(&self) -> OperationId {
        self.identity.operation_id
    }

    pub fn class(&self) -> BlockingDispatchClass {
        self.class
    }

    pub fn run(mut self) -> ExecutorDriveReport {
        let start = self.start.take().expect("dispatch owns start token");
        let decision = start.claim_for_run();
        contained_drop(start);
        if decision == ExecutorStartDecision::CompleteCancelled {
            let result = Err(ExternalFailure::cancelled());
            let terminal = terminal_for(&result);
            let delivery = self
                .sink
                .take()
                .expect("dispatch owns sink")
                .deliver(result);
            if let Some(job) = self.job.take() {
                contained_drop(job);
            }
            return ExecutorDriveReport { terminal, delivery };
        }
        let result = match decision {
            ExecutorStartDecision::CompleteCancelled => unreachable!(),
            ExecutorStartDecision::Run => run_blocking(self.job.take().expect("dispatch owns job")),
        };
        let terminal = terminal_for(&result);
        let delivery = self
            .sink
            .take()
            .expect("dispatch owns sink")
            .deliver(result);
        ExecutorDriveReport { terminal, delivery }
    }
}

impl Drop for BlockingExecutorDispatch {
    fn drop(&mut self) {
        if let Some(sink) = self.sink.take() {
            let _ = sink.deliver(Err(ExternalFailure::cancelled()));
        }
        if let Some(job) = self.job.take() {
            contained_drop(job);
        }
        if let Some(start) = self.start.take() {
            contained_drop(start);
        }
    }
}

fn run_blocking(job: BlockingJob) -> JobResult {
    #[cfg(panic = "unwind")]
    {
        catch_unwind(AssertUnwindSafe(job)).unwrap_or_else(|_| Err(ExternalFailure::worker_panic()))
    }
    #[cfg(panic = "abort")]
    {
        job()
    }
}

pub struct AsyncExecutorDispatch {
    identity: CompletionIdentity,
    sink: Option<CompletionSink>,
    start: Option<ExecutorStartToken>,
    job: Option<AsyncJob>,
}

impl AsyncExecutorDispatch {
    pub fn operation_id(&self) -> OperationId {
        self.identity.operation_id
    }

    pub fn into_future(mut self) -> AsyncDispatchFuture {
        let start = self.start.take().expect("dispatch owns start token");
        let decision = start.claim_for_run();
        contained_drop(start);
        let future = match decision {
            ExecutorStartDecision::CompleteCancelled => AsyncFutureState::Immediate {
                result: Some(Err(ExternalFailure::cancelled())),
                unstarted_job: self.job.take(),
            },
            ExecutorStartDecision::Run => {
                construct_async(self.job.take().expect("dispatch owns job"))
            }
        };
        AsyncDispatchFuture {
            identity: self.identity,
            sink: self.sink.take(),
            future,
        }
    }
}

impl Drop for AsyncExecutorDispatch {
    fn drop(&mut self) {
        if let Some(sink) = self.sink.take() {
            let _ = sink.deliver(Err(ExternalFailure::cancelled()));
        }
        if let Some(job) = self.job.take() {
            contained_drop(job);
        }
        if let Some(start) = self.start.take() {
            contained_drop(start);
        }
    }
}

enum AsyncFutureState {
    Running(AsyncJobFuture),
    Immediate {
        result: Option<JobResult>,
        unstarted_job: Option<AsyncJob>,
    },
    Done,
}

fn construct_async(job: AsyncJob) -> AsyncFutureState {
    #[cfg(panic = "unwind")]
    {
        match catch_unwind(AssertUnwindSafe(job)) {
            Ok(future) => AsyncFutureState::Running(future),
            Err(_) => AsyncFutureState::Immediate {
                result: Some(Err(ExternalFailure::worker_panic())),
                unstarted_job: None,
            },
        }
    }
    #[cfg(panic = "abort")]
    {
        AsyncFutureState::Running(job())
    }
}

pub struct AsyncDispatchFuture {
    identity: CompletionIdentity,
    sink: Option<CompletionSink>,
    future: AsyncFutureState,
}

impl AsyncDispatchFuture {
    pub fn operation_id(&self) -> OperationId {
        self.identity.operation_id
    }
}

impl Future for AsyncDispatchFuture {
    type Output = ExecutorDriveReport;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let result = match &mut self.future {
            AsyncFutureState::Running(future) => {
                #[cfg(panic = "unwind")]
                let polled = catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(context)));
                #[cfg(panic = "unwind")]
                match polled {
                    Ok(Poll::Pending) => return Poll::Pending,
                    Ok(Poll::Ready(result)) => result,
                    Err(_) => Err(ExternalFailure::worker_panic()),
                }
                #[cfg(panic = "abort")]
                match future.as_mut().poll(context) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(result) => result,
                }
            }
            AsyncFutureState::Immediate { result, .. } => {
                result.take().expect("immediate result is polled once")
            }
            AsyncFutureState::Done => panic!("completed dispatch future polled again"),
        };
        let terminal = terminal_for(&result);
        let delivery = self.sink.take().expect("future owns sink").deliver(result);
        let old = std::mem::replace(&mut self.future, AsyncFutureState::Done);
        contained_drop_async_state(old);
        Poll::Ready(ExecutorDriveReport { terminal, delivery })
    }
}

impl Drop for AsyncDispatchFuture {
    fn drop(&mut self) {
        if let Some(sink) = self.sink.take() {
            let _ = sink.deliver(Err(ExternalFailure::cancelled()));
        }
        let old = std::mem::replace(&mut self.future, AsyncFutureState::Done);
        contained_drop_async_state(old);
    }
}

fn contained_drop_async_state(state: AsyncFutureState) {
    #[cfg(panic = "unwind")]
    if std::thread::panicking() {
        std::mem::forget(state);
        return;
    }
    match state {
        AsyncFutureState::Running(future) => contained_drop(future),
        AsyncFutureState::Immediate {
            result,
            unstarted_job,
        } => {
            contained_drop(result);
            if let Some(job) = unstarted_job {
                contained_drop(job);
            }
        }
        AsyncFutureState::Done => {}
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitErrorKind {
    Capacity,
    ShuttingDown,
    RuntimeDetached,
}

pub struct SubmissionRejected {
    kind: SubmitErrorKind,
    submission: Box<ExecutorSubmission>,
}

impl SubmissionRejected {
    pub fn kind(&self) -> SubmitErrorKind {
        self.kind
    }

    pub fn operation_id(&self) -> OperationId {
        self.submission.operation_id()
    }

    pub fn rollback(self) -> SubmitErrorKind {
        let SubmissionRejected { kind, submission } = self;
        let mut submission = *submission;
        if let Some(sink) = submission.sink.take() {
            contained_drop(sink);
        }
        if let Some(job) = submission.job.take() {
            contained_drop(job);
        }
        contained_drop(submission.start);
        kind
    }
}

fn contained_drop<T>(value: T) {
    #[cfg(panic = "unwind")]
    {
        // Starting an opaque destructor during an active unwind could abort on a
        // second panic. Leak it instead. A single destructor panic outside an
        // unwind is caught; a destructor that double-panics internally is fatal.
        if std::thread::panicking() {
            std::mem::forget(value);
            return;
        }
        let _ = catch_unwind(AssertUnwindSafe(|| drop(value)));
    }
    #[cfg(panic = "abort")]
    {
        drop(value);
    }
}

fn destroy_prepared(prepared: PreparedExternalOperation) {
    let PreparedExternalOperation {
        decoder,
        resource,
        job,
        ..
    } = prepared;
    contained_drop(decoder);
    contained_drop(resource);
    contained_drop(job);
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExecutorSnapshot {
    pub queued: usize,
    pub running_interruptible: usize,
    pub running_quarantined: usize,
    pub completed: usize,
    pub cancelled: usize,
    pub panicked: usize,
    pub undeliverable: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutorShutdown {
    Drained(ExecutorSnapshot),
    DeadlineExceeded(ExecutorSnapshot),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunningSubmission {
    operation_id: OperationId,
}
impl RunningSubmission {
    pub fn new(operation_id: OperationId) -> Self {
        Self { operation_id }
    }
    pub fn operation_id(&self) -> OperationId {
        self.operation_id
    }
}

pub trait ExecutorLease: Send + Sync + 'static {
    /// Reserve capacity before arming with `into_dispatch`; enqueue only the armed dispatch.
    fn submit(
        &self,
        submission: ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected>;
    fn snapshot(&self) -> ExecutorSnapshot;
    fn shutdown(&self, deadline: Instant) -> ExecutorShutdown;
}

pub trait IoExecutor: Send + Sync + 'static {
    fn attach_runtime(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Arc<dyn ExecutorLease>, ExecutorAttachError>;
    fn snapshot(&self) -> ExecutorSnapshot;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutorAttachError {
    DuplicateRuntime { runtime_id: RuntimeId },
    ShuttingDown,
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::num::NonZeroU64;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::task::Waker;

    use crate::cycle::GcEdge;
    use crate::runtime::{
        CancelDisposition, CancelHook, CancelHookError, CancellationView, CompletionDecoder,
        ExternalFailureCode, NativeCallContext, TaskContext, Trace,
    };
    use crate::Value;

    use super::*;

    struct RecordingSender {
        completions: Mutex<Vec<ExternalCompletion>>,
        attempts: AtomicUsize,
        delivery: CompletionDelivery,
    }

    impl RecordingSender {
        fn new(delivery: CompletionDelivery) -> Arc<Self> {
            Arc::new(Self {
                completions: Mutex::new(Vec::new()),
                attempts: AtomicUsize::new(0),
                delivery,
            })
        }

        fn take(&self) -> ExternalCompletion {
            self.completions.lock().unwrap().pop().unwrap()
        }
    }

    impl CompletionSender for RecordingSender {
        fn send(&self, completion: ExternalCompletion) -> CompletionDelivery {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            self.completions.lock().unwrap().push(completion);
            self.delivery
        }
    }

    struct LocalDecoder(Rc<Cell<usize>>);

    impl Trace for LocalDecoder {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl CompletionDecoder for LocalDecoder {
        fn decode(
            self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            result: JobResult,
        ) -> Result<Value, crate::SemaError> {
            self.0.set(self.0.get() + 1);
            result
                .map(|_| Value::int(1))
                .map_err(|failure| crate::SemaError::eval(failure.message()))
        }
    }

    struct LocalHook(Rc<Cell<usize>>);

    impl Trace for LocalHook {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl CancelHook for LocalHook {
        fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
            self.0.set(self.0.get() + 1);
            Ok(CancelDisposition::Reaped)
        }

        fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
            Ok(CancelDisposition::Reaped)
        }
    }

    #[test]
    fn prepared_operation_traces_decoder_then_interruptible_hook_never_job() {
        struct EdgeDecoder(Value);
        impl Trace for EdgeDecoder {
            fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
                sink(GcEdge::Value(&self.0));
                true
            }
        }
        impl CompletionDecoder for EdgeDecoder {
            fn decode(
                self: Box<Self>,
                _context: &mut NativeCallContext<'_>,
                _result: JobResult,
            ) -> Result<Value, crate::SemaError> {
                Ok(self.0)
            }
        }
        struct EdgeHook(Value);
        impl Trace for EdgeHook {
            fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
                sink(GcEdge::Value(&self.0));
                true
            }
        }
        impl CancelHook for EdgeHook {
            fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
                Ok(CancelDisposition::Reaped)
            }
            fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
                Ok(CancelDisposition::Reaped)
            }
        }

        let value = Value::string("duplicate");
        let prepared = PreparedExternalOperation::interruptible_blocking(
            kind(7),
            Box::new(EdgeDecoder(value.clone())),
            InterruptibleResource::new("edge", Box::new(EdgeHook(value))),
            || Ok(Box::new(1_u8)),
        );
        assert_eq!(prepared.completion_kind(), kind(7));
        let mut edges = 0;
        assert!(prepared.trace(&mut |_| edges += 1));
        assert_eq!(edges, 2, "decoder and hook each own one duplicate edge");

        let quarantined = PreparedExternalOperation::quarantined_blocking(
            kind(8),
            Box::new(EdgeDecoder(Value::NIL)),
            QuarantineBound::finite_work("unit", NonZeroU64::new(1).unwrap()),
            || Ok(Box::new(1_u8)),
        );
        let mut edges = 0;
        assert!(quarantined.trace(&mut |_| edges += 1));
        assert_eq!(edges, 1, "quarantined resources have no edges");

        struct BorrowingHook {
            first: Value,
            second: Rc<RefCell<Value>>,
        }
        impl Trace for BorrowingHook {
            fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
                sink(GcEdge::Value(&self.first));
                match self.second.try_borrow() {
                    Ok(second) => {
                        sink(GcEdge::Value(&second));
                        true
                    }
                    Err(_) => false,
                }
            }
        }
        impl CancelHook for BorrowingHook {
            fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
                Ok(CancelDisposition::Reaped)
            }
            fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
                Ok(CancelDisposition::Reaped)
            }
        }
        let second = Rc::new(RefCell::new(Value::NIL));
        let borrow = second.borrow_mut();
        let failing = PreparedExternalOperation::interruptible_blocking(
            kind(9),
            Box::new(EdgeDecoder(Value::NIL)),
            InterruptibleResource::new(
                "borrowed",
                Box::new(BorrowingHook {
                    first: Value::NIL,
                    second: Rc::clone(&second),
                }),
            ),
            || Ok(Box::new(1_u8)),
        );
        let mut edges = 0;
        assert!(!failing.trace(&mut |_| edges += 1));
        assert_eq!(edges, 2, "decoder and hook output remains before failure");
        drop(borrow);
    }

    fn interruptible() -> InterruptibleResource {
        InterruptibleResource::new("test", Box::new(LocalHook(Rc::new(Cell::new(0)))))
    }

    fn kind(raw: u64) -> CompletionKind {
        CompletionKind::try_from_raw(raw).unwrap()
    }

    fn decoder() -> Box<dyn CompletionDecoder> {
        Box::new(LocalDecoder(Rc::new(Cell::new(0))))
    }

    fn registration(
        prepared: PreparedExternalOperation,
        selected_kind: CompletionKind,
        delivery: CompletionDelivery,
    ) -> (
        Arc<RecordingSender>,
        RuntimeOperationBinding,
        ExecutorSubmission,
        CompletionIdentity,
    ) {
        let sender = RecordingSender::new(delivery);
        let (_, registrar) = CompletionRegistrar::register(sender.clone()).unwrap();
        let identity = registrar.issue_identity(selected_kind).unwrap();
        let descriptor = CompletionIdentity {
            runtime_id: identity.runtime_id(),
            wait_id: identity.wait_id(),
            generation: identity.generation(),
            operation_id: identity.operation_id(),
            kind: identity.kind(),
        };
        let (runtime, submission) = registrar.bind(identity, prepared).unwrap().split();
        (sender, runtime, submission, descriptor)
    }

    fn blocking(result: JobResult) -> PreparedExternalOperation {
        PreparedExternalOperation::interruptible_blocking(
            kind(1),
            decoder(),
            interruptible(),
            move || result,
        )
    }

    fn poll_once(future: &mut AsyncDispatchFuture) -> Poll<ExecutorDriveReport> {
        let mut context = Context::from_waker(Waker::noop());
        Pin::new(future).poll(&mut context)
    }

    #[test]
    fn send_halves_are_send_while_decoder_and_resource_accept_rc() {
        fn assert_send<T: Send>() {}
        assert_send::<ExternalCompletion>();
        assert_send::<ExecutorSubmission>();
        assert_send::<ExecutorDispatch>();
        assert_send::<AsyncExecutorDispatch>();
        assert_send::<BlockingExecutorDispatch>();
        assert_send::<AsyncDispatchFuture>();

        let local = Rc::new(Cell::new(0));
        let prepared = PreparedExternalOperation::interruptible_blocking(
            kind(1),
            Box::new(LocalDecoder(Rc::clone(&local))),
            InterruptibleResource::new("local", Box::new(LocalHook(Rc::clone(&local)))),
            || Ok(Box::new(1_u8)),
        );
        let (_sender, runtime, _submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        drop(runtime);
    }

    #[test]
    fn resource_wrapper_cancels_once_and_reaps_repeatedly() {
        struct CountingHook {
            cancel: Rc<Cell<usize>>,
            reap: Rc<Cell<usize>>,
        }
        impl Trace for CountingHook {
            fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
                true
            }
        }
        impl CancelHook for CountingHook {
            fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
                self.cancel.set(self.cancel.get() + 1);
                Ok(CancelDisposition::PendingReap)
            }
            fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
                self.reap.set(self.reap.get() + 1);
                Ok(CancelDisposition::PendingReap)
            }
        }
        let cancel = Rc::new(Cell::new(0));
        let reap = Rc::new(Cell::new(0));
        let mut resource = ResourceClass::interruptible(
            "socket",
            Box::new(CountingHook {
                cancel: Rc::clone(&cancel),
                reap: Rc::clone(&reap),
            }),
        );
        assert_eq!(resource.kind(), "socket");
        assert_eq!(resource.bound(), None);
        assert!(resource.cancel().unwrap().is_ok());
        assert!(resource.cancel().is_none());
        assert!(resource.reap().unwrap().is_ok());
        assert!(resource.reap().unwrap().is_ok());
        assert_eq!(cancel.get(), 1);
        assert_eq!(reap.get(), 2);

        let bound = QuarantineBound::finite_work("items", std::num::NonZeroU64::new(2).unwrap());
        let descriptor = bound.descriptor();
        let mut quarantined = ResourceClass::quarantined(bound);
        assert_eq!(quarantined.bound(), Some(descriptor));
        assert!(quarantined.cancel().is_none());
        assert!(quarantined.reap().is_none());
    }

    #[test]
    fn decoder_consumes_typed_result_on_runtime_thread() {
        let count = Rc::new(Cell::new(0));
        let decoder: Box<dyn CompletionDecoder> = Box::new(LocalDecoder(Rc::clone(&count)));
        let mut task_context = TaskContext::empty();
        let mut context = NativeCallContext {
            task_context: &mut task_context,
            cancellation: CancellationView::default(),
        };
        assert_eq!(
            decoder.decode(&mut context, Ok(Box::new(1_u8))).unwrap(),
            Value::int(1)
        );
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn all_prepared_constructor_classes_bind() {
        let bound = QuarantineBound::finite_work("items", std::num::NonZeroU64::new(1).unwrap());
        let prepared = [
            PreparedExternalOperation::interruptible_async(
                kind(1),
                decoder(),
                interruptible(),
                || async { Ok(Box::new(1_u8) as SendPayload) },
            ),
            PreparedExternalOperation::interruptible_blocking(
                kind(1),
                decoder(),
                interruptible(),
                || Ok(Box::new(1_u8)),
            ),
            PreparedExternalOperation::quarantined_blocking(kind(1), decoder(), bound, || {
                Ok(Box::new(1_u8))
            }),
        ];
        for operation in prepared {
            let declared_kind = operation.kind;
            let (_sender, runtime, _submission, _) =
                registration(operation, declared_kind, CompletionDelivery::Delivered);
            let (_, resource, _) = runtime.into_parts();
            assert!(resource.kind() == "test" || resource.bound().is_some());
        }
    }

    #[test]
    fn fresh_registrars_reject_foreign_identity() {
        let sender = RecordingSender::new(CompletionDelivery::Delivered);
        let (first_id, first) = CompletionRegistrar::register(sender.clone()).unwrap();
        let (second_id, second) = CompletionRegistrar::register(sender).unwrap();
        assert_ne!(first_id, second_id);
        let identity = first.issue_identity(kind(1)).unwrap();
        assert!(second.bind(identity, blocking(Ok(Box::new(1_u8)))).is_err());
    }

    #[test]
    fn registrar_rejects_identity_kind_mismatch() {
        let sender = RecordingSender::new(CompletionDelivery::Delivered);
        let (_, registrar) = CompletionRegistrar::register(sender.clone()).unwrap();
        let identity = registrar.issue_identity(kind(7)).unwrap();
        let error = match registrar.bind(identity, blocking(Ok(Box::new(1_u8)))) {
            Err(error) => error,
            Ok(_) => panic!("mismatched completion kinds must be rejected"),
        };
        assert_eq!(error.expected(), kind(7));
        assert_eq!(error.declared(), kind(1));
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn matching_identity_kind_delivers_declared_kind() {
        let (sender, _runtime, submission, identity) = registration(
            blocking(Ok(Box::new(1_u8))),
            kind(1),
            CompletionDelivery::Delivered,
        );
        let ExecutorDispatch::Blocking(dispatch) = submission.into_dispatch() else {
            panic!("blocking dispatch expected")
        };
        dispatch.run();
        let completion = sender.take();
        assert_eq!(completion.runtime_id, identity.runtime_id);
        assert_eq!(completion.wait_id, identity.wait_id);
        assert_eq!(completion.generation, identity.generation);
        assert_eq!(completion.operation_id, identity.operation_id);
        assert_eq!(completion.kind, kind(1));
    }

    #[test]
    fn identity_is_consumed_by_bind() {
        fn bind_once(
            registrar: &CompletionRegistrar,
            identity: RuntimeIssuedCompletionIdentity,
            prepared: PreparedExternalOperation,
        ) {
            let _ = registrar.bind(identity, prepared);
            // A second bind cannot be expressed because `identity` has moved.
        }

        let sender = RecordingSender::new(CompletionDelivery::Delivered);
        let (_, registrar) = CompletionRegistrar::register(sender).unwrap();
        bind_once(
            &registrar,
            registrar.issue_identity(kind(1)).unwrap(),
            blocking(Ok(Box::new(1_u8))),
        );
    }

    #[test]
    fn rejection_is_silent_and_destroys_job_internally() {
        let dropped = Arc::new(AtomicUsize::new(0));
        struct DropCount(Arc<AtomicUsize>);
        impl Drop for DropCount {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let captured = DropCount(Arc::clone(&dropped));
        let prepared = PreparedExternalOperation::interruptible_blocking(
            kind(1),
            decoder(),
            interruptible(),
            move || {
                drop(captured);
                Ok(Box::new(1_u8))
            },
        );
        let (sender, _runtime, submission, identity) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let rejection = submission.reject(SubmitErrorKind::Capacity);
        assert_eq!(rejection.operation_id(), identity.operation_id);
        assert_eq!(rejection.rollback(), SubmitErrorKind::Capacity);
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 0);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cancel_before_start_prevents_blocking_body() {
        let ran = Arc::new(AtomicUsize::new(0));
        let ran_job = Arc::clone(&ran);
        let prepared = PreparedExternalOperation::interruptible_blocking(
            kind(1),
            decoder(),
            interruptible(),
            move || {
                ran_job.fetch_add(1, Ordering::Relaxed);
                Ok(Box::new(1_u8))
            },
        );
        let (sender, runtime, submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let (_, _, cancel) = runtime.into_parts();
        assert_eq!(
            cancel.cancel_before_start(),
            CancelBeforeStart::CancelledQueued
        );
        let ExecutorDispatch::Blocking(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        dispatch.run();
        assert_eq!(ran.load(Ordering::Relaxed), 0);
        assert_eq!(
            sender.take().result.unwrap_err().code(),
            ExternalFailureCode::Cancelled
        );
    }

    #[test]
    fn start_before_cancel_runs_body() {
        let (sender, runtime, submission, _) = registration(
            blocking(Ok(Box::new(4_u8))),
            kind(1),
            CompletionDelivery::Delivered,
        );
        let (_, _, cancel) = runtime.into_parts();
        let ExecutorDispatch::Blocking(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        assert_eq!(dispatch.run().delivery, CompletionDelivery::Delivered);
        assert_eq!(
            cancel.cancel_before_start(),
            CancelBeforeStart::AlreadyRunning
        );
        assert!(sender.take().result.is_ok());
    }

    #[test]
    fn blocking_return_error_and_panic_each_deliver_once() {
        let cases = [
            blocking(Ok(Box::new(1_u8))),
            blocking(Err(ExternalFailure::bound_exceeded("bound"))),
            blocking_panic(),
        ];
        for (index, prepared) in cases.into_iter().enumerate() {
            let (sender, _runtime, submission, _) =
                registration(prepared, kind(1), CompletionDelivery::Delivered);
            let ExecutorDispatch::Blocking(dispatch) = submission.into_dispatch() else {
                unreachable!()
            };
            dispatch.run();
            assert_eq!(sender.attempts.load(Ordering::Relaxed), 1, "case {index}");
        }
    }

    #[cfg(panic = "unwind")]
    fn blocking_panic() -> PreparedExternalOperation {
        PreparedExternalOperation::interruptible_blocking(
            kind(1),
            decoder(),
            interruptible(),
            || panic!("worker panic"),
        )
    }

    #[cfg(panic = "abort")]
    fn blocking_panic() -> PreparedExternalOperation {
        blocking(Err(ExternalFailure::worker_panic()))
    }

    #[test]
    fn dispatch_drop_is_post_arm_cancellation_not_rejection() {
        let (sender, _runtime, submission, _) = registration(
            blocking(Ok(Box::new(1_u8))),
            kind(1),
            CompletionDelivery::Delivered,
        );
        drop(submission.into_dispatch());
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
        assert_eq!(
            sender.take().result.unwrap_err().code(),
            ExternalFailureCode::Cancelled
        );
    }

    #[test]
    fn async_return_and_construction_panic_deliver_once() {
        let normal = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            || async { Ok(Box::new(1_u8) as SendPayload) },
        );
        let returned_error = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            || async { Err(ExternalFailure::deadline_exceeded("deadline")) },
        );
        #[cfg(panic = "unwind")]
        let panics = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            || -> std::future::Ready<JobResult> { panic!("construction panic") },
        );
        #[cfg(panic = "abort")]
        let panics = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            || std::future::ready(Err(ExternalFailure::worker_panic())),
        );

        for prepared in [normal, returned_error, panics] {
            let (sender, _runtime, submission, _) =
                registration(prepared, kind(1), CompletionDelivery::Delivered);
            let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
                unreachable!()
            };
            let mut future = dispatch.into_future();
            let Poll::Ready(report) = poll_once(&mut future) else {
                panic!("fixture is immediately ready")
            };
            assert_eq!(report.delivery, CompletionDelivery::Delivered);
            assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn async_queued_cancellation_does_not_construct_future() {
        let constructed = Arc::new(AtomicUsize::new(0));
        let job_constructed = Arc::clone(&constructed);
        let prepared = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            move || {
                job_constructed.fetch_add(1, Ordering::Relaxed);
                std::future::ready(Ok(Box::new(1_u8) as SendPayload))
            },
        );
        let (sender, runtime, submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let (_, _, cancel) = runtime.into_parts();
        assert_eq!(
            cancel.cancel_before_start(),
            CancelBeforeStart::CancelledQueued
        );
        let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        let mut future = dispatch.into_future();
        assert!(poll_once(&mut future).is_ready());
        assert_eq!(constructed.load(Ordering::Relaxed), 0);
        assert_eq!(
            sender.take().result.unwrap_err().code(),
            ExternalFailureCode::Cancelled
        );
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn async_poll_panic_maps_worker_panic_once() {
        let prepared = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            || async {
                panic!("poll panic");
                #[allow(unreachable_code)]
                Ok(Box::new(1_u8) as SendPayload)
            },
        );
        let (sender, _runtime, submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        let mut future = dispatch.into_future();
        assert!(matches!(poll_once(&mut future), Poll::Ready(_)));
        assert_eq!(
            sender.take().result.unwrap_err().code(),
            ExternalFailureCode::WorkerPanic
        );
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn async_future_drop_before_and_after_pending_delivers_once() {
        let retained_waker = Arc::new(Mutex::new(None::<Waker>));
        let waker_slot = Arc::clone(&retained_waker);
        let pending = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            move || {
                std::future::poll_fn(move |cx| {
                    *waker_slot.lock().unwrap() = Some(cx.waker().clone());
                    Poll::Pending
                })
            },
        );
        let (sender, _runtime, submission, _) =
            registration(pending, kind(1), CompletionDelivery::Delivered);
        let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        let mut future = dispatch.into_future();
        assert!(poll_once(&mut future).is_pending());
        drop(future);
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);

        let ready = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            || async { Ok(Box::new(1_u8) as SendPayload) },
        );
        let (sender, _runtime, submission, _) =
            registration(ready, kind(1), CompletionDelivery::Delivered);
        let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        drop(dispatch.into_future());
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn closed_inbox_is_accounted_by_sender_once() {
        let (sender, _runtime, submission, _) = registration(
            blocking(Ok(Box::new(1_u8))),
            kind(1),
            CompletionDelivery::InboxClosed,
        );
        let ExecutorDispatch::Blocking(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        assert_eq!(dispatch.run().delivery, CompletionDelivery::InboxClosed);
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn queue_control_has_deterministic_cas_outcomes() {
        let state = Arc::new(AtomicU8::new(QUEUED));
        let cancel = ExecutorCancelHandle {
            state: Arc::clone(&state),
        };
        let start = ExecutorStartToken { state };
        assert_eq!(
            cancel.cancel_before_start(),
            CancelBeforeStart::CancelledQueued
        );
        assert_eq!(
            start.claim_for_run(),
            ExecutorStartDecision::CompleteCancelled
        );

        let state = Arc::new(AtomicU8::new(QUEUED));
        let cancel = ExecutorCancelHandle {
            state: Arc::clone(&state),
        };
        let start = ExecutorStartToken { state };
        assert_eq!(start.claim_for_run(), ExecutorStartDecision::Run);
        assert_eq!(
            cancel.cancel_before_start(),
            CancelBeforeStart::AlreadyRunning
        );
    }

    #[test]
    fn runtime_binding_decoder_can_be_invoked_after_split() {
        let count = Rc::new(Cell::new(0));
        let prepared = PreparedExternalOperation::interruptible_blocking(
            kind(1),
            Box::new(LocalDecoder(Rc::clone(&count))),
            interruptible(),
            || Ok(Box::new(1_u8)),
        );
        let (_sender, runtime, _submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let (decoder, _, _) = runtime.into_parts();
        let mut task = TaskContext::empty();
        let mut context = NativeCallContext {
            task_context: &mut task,
            cancellation: CancellationView::default(),
        };
        decoder.decode(&mut context, Ok(Box::new(1_u8))).unwrap();
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn no_duplicate_after_async_ready() {
        let prepared = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            || async { Ok(Box::new(1_u8) as SendPayload) },
        );
        let (sender, _runtime, submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        let mut future = dispatch.into_future();
        assert!(poll_once(&mut future).is_ready());
        drop(future);
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn ready_result_is_delivered_before_panicking_future_drop() {
        struct HostileFuture;
        impl Future for HostileFuture {
            type Output = JobResult;
            fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                Poll::Ready(Ok(Box::new(1_u8)))
            }
        }
        impl Drop for HostileFuture {
            fn drop(&mut self) {
                panic!("hostile future drop");
            }
        }

        let prepared = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            || HostileFuture,
        );
        let (sender, _runtime, submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        let mut future = dispatch.into_future();
        assert!(catch_unwind(AssertUnwindSafe(|| poll_once(&mut future))).is_ok());
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
        assert!(sender.take().result.is_ok());
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn poll_panic_is_delivered_before_panicking_future_drop() {
        struct HostileFuture;
        impl Future for HostileFuture {
            type Output = JobResult;
            fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                panic!("hostile poll");
            }
        }
        impl Drop for HostileFuture {
            fn drop(&mut self) {
                panic!("hostile future drop");
            }
        }

        let prepared = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            || HostileFuture,
        );
        let (sender, _runtime, submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        let mut future = dispatch.into_future();
        assert!(catch_unwind(AssertUnwindSafe(|| poll_once(&mut future))).is_ok());
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
        assert_eq!(
            sender.take().result.unwrap_err().code(),
            ExternalFailureCode::WorkerPanic
        );
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn pending_abandonment_delivers_before_containing_future_drop_panic() {
        struct HostilePending;
        impl Future for HostilePending {
            type Output = JobResult;
            fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                Poll::Pending
            }
        }
        impl Drop for HostilePending {
            fn drop(&mut self) {
                panic!("hostile pending drop");
            }
        }
        let prepared = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            || HostilePending,
        );
        let (sender, _runtime, submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        let mut future = dispatch.into_future();
        assert!(poll_once(&mut future).is_pending());
        assert!(catch_unwind(AssertUnwindSafe(|| drop(future))).is_ok());
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
        assert_eq!(
            sender.take().result.unwrap_err().code(),
            ExternalFailureCode::Cancelled
        );
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn queued_blocking_cancellation_contains_job_drop_panic() {
        struct HostileDrop;
        impl Drop for HostileDrop {
            fn drop(&mut self) {
                panic!("hostile job drop");
            }
        }
        let hostile = HostileDrop;
        let prepared = PreparedExternalOperation::interruptible_blocking(
            kind(1),
            decoder(),
            interruptible(),
            move || {
                drop(hostile);
                Ok(Box::new(1_u8))
            },
        );
        let (sender, runtime, submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let (_, _, cancel) = runtime.into_parts();
        assert_eq!(
            cancel.cancel_before_start(),
            CancelBeforeStart::CancelledQueued
        );
        let ExecutorDispatch::Blocking(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        assert!(catch_unwind(AssertUnwindSafe(|| dispatch.run())).is_ok());
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
        assert_eq!(
            sender.take().result.unwrap_err().code(),
            ExternalFailureCode::Cancelled
        );
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn rejection_rollback_contains_owned_destructor_panic() {
        struct HostileDrop;
        impl Drop for HostileDrop {
            fn drop(&mut self) {
                panic!("hostile rollback drop");
            }
        }
        let hostile = HostileDrop;
        let prepared = PreparedExternalOperation::interruptible_blocking(
            kind(1),
            decoder(),
            interruptible(),
            move || {
                drop(hostile);
                Ok(Box::new(1_u8))
            },
        );
        let (sender, _runtime, submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let result = catch_unwind(AssertUnwindSafe(|| {
            submission.reject(SubmitErrorKind::Capacity).rollback()
        }));
        assert_eq!(result.unwrap(), SubmitErrorKind::Capacity);
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 0);
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn queued_async_cancellation_delivers_before_containing_job_drop_panic() {
        struct HostileDrop;
        impl Drop for HostileDrop {
            fn drop(&mut self) {
                panic!("hostile queued async job drop");
            }
        }
        let hostile = HostileDrop;
        let prepared = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            move || {
                drop(hostile);
                std::future::ready(Ok(Box::new(1_u8) as SendPayload))
            },
        );
        let (sender, runtime, submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let (_, _, cancel) = runtime.into_parts();
        assert_eq!(
            cancel.cancel_before_start(),
            CancelBeforeStart::CancelledQueued
        );
        let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        let mut future = dispatch.into_future();
        let report = catch_unwind(AssertUnwindSafe(|| poll_once(&mut future)))
            .expect("single destructor panic is contained");
        assert_eq!(
            report,
            Poll::Ready(ExecutorDriveReport {
                terminal: ExecutorTerminal::Cancelled,
                delivery: CompletionDelivery::Delivered,
            })
        );
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);

        let hostile = HostileDrop;
        let prepared = PreparedExternalOperation::interruptible_async(
            kind(1),
            decoder(),
            interruptible(),
            move || {
                drop(hostile);
                std::future::ready(Ok(Box::new(1_u8) as SendPayload))
            },
        );
        let (sender, runtime, submission, _) =
            registration(prepared, kind(1), CompletionDelivery::Delivered);
        let (_, _, cancel) = runtime.into_parts();
        assert_eq!(
            cancel.cancel_before_start(),
            CancelBeforeStart::CancelledQueued
        );
        let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
            unreachable!()
        };
        let future = dispatch.into_future();
        assert!(catch_unwind(AssertUnwindSafe(|| drop(future))).is_ok());
        assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
        assert_eq!(
            sender.take().result.unwrap_err().code(),
            ExternalFailureCode::Cancelled
        );
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn armed_dispatch_drop_delivers_before_containing_job_drop_panic() {
        struct HostileDrop;
        impl Drop for HostileDrop {
            fn drop(&mut self) {
                panic!("hostile armed job drop");
            }
        }
        for asynchronous in [false, true] {
            let hostile = HostileDrop;
            let prepared = if asynchronous {
                PreparedExternalOperation::interruptible_async(
                    kind(1),
                    decoder(),
                    interruptible(),
                    move || {
                        drop(hostile);
                        std::future::ready(Ok(Box::new(1_u8) as SendPayload))
                    },
                )
            } else {
                PreparedExternalOperation::interruptible_blocking(
                    kind(1),
                    decoder(),
                    interruptible(),
                    move || {
                        drop(hostile);
                        Ok(Box::new(1_u8))
                    },
                )
            };
            let (sender, _runtime, submission, _) =
                registration(prepared, kind(1), CompletionDelivery::Delivered);
            assert!(catch_unwind(AssertUnwindSafe(|| drop(submission.into_dispatch()))).is_ok());
            assert_eq!(sender.attempts.load(Ordering::Relaxed), 1);
            assert_eq!(
                sender.take().result.unwrap_err().code(),
                ExternalFailureCode::Cancelled
            );
        }
    }

    #[test]
    fn terminal_report_classifies_worker_results_without_exposing_payload() {
        let cases = [
            (blocking(Ok(Box::new(1_u8))), ExecutorTerminal::Completed),
            (
                blocking(Err(ExternalFailure::bound_exceeded("bound"))),
                ExecutorTerminal::Completed,
            ),
            (blocking_panic(), ExecutorTerminal::WorkerPanic),
        ];
        for (prepared, terminal) in cases {
            let (_sender, _runtime, submission, _) =
                registration(prepared, kind(1), CompletionDelivery::Delivered);
            let ExecutorDispatch::Blocking(dispatch) = submission.into_dispatch() else {
                unreachable!()
            };
            assert_eq!(
                dispatch.run(),
                ExecutorDriveReport {
                    terminal,
                    delivery: CompletionDelivery::Delivered,
                }
            );
        }
    }
}
