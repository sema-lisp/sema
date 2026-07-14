use std::cell::RefCell;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use super::{
    CompletionDecoder, CompletionDelivery, CompletionKind, CompletionSender, ExternalCompletion,
    ExternalFailure, IdCounter, IdExhausted, InterruptibleResource, OperationId, QuarantineBound,
    ResourceClass, RuntimeId, SendPayload, WaitGeneration, WaitId,
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
            resource: ResourceClass::Interruptible {
                kind: resource_kind,
                hook,
            },
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
            resource: ResourceClass::Interruptible {
                kind: resource_kind,
                hook,
            },
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
            resource: ResourceClass::QuarantinedBounded(bound),
            job: PreparedJob::Blocking {
                class: BlockingDispatchClass::QuarantinedBounded,
                job: Box::new(job),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeIssuedCompletionIdentity {
    runtime_id: RuntimeId,
    wait_id: WaitId,
    generation: WaitGeneration,
    operation_id: OperationId,
    kind: CompletionKind,
    authority: u64,
}

impl RuntimeIssuedCompletionIdentity {
    pub fn runtime_id(self) -> RuntimeId {
        self.runtime_id
    }

    pub fn wait_id(self) -> WaitId {
        self.wait_id
    }

    pub fn generation(self) -> WaitGeneration {
        self.generation
    }

    pub fn operation_id(self) -> OperationId {
        self.operation_id
    }

    pub fn kind(self) -> CompletionKind {
        self.kind
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("completion identity was issued by a different registrar")]
pub struct ForeignCompletionIdentity;

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
    ) -> Result<ExternalOperationBinding, ForeignCompletionIdentity> {
        if identity.runtime_id != self.runtime_id || identity.authority != self.authority {
            return Err(ForeignCompletionIdentity);
        }
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
    identity: RuntimeIssuedCompletionIdentity,
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
    identity: RuntimeIssuedCompletionIdentity,
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
            submission: self,
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

pub struct BlockingExecutorDispatch {
    identity: RuntimeIssuedCompletionIdentity,
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

    pub fn run(mut self) -> CompletionDelivery {
        let decision = self
            .start
            .take()
            .expect("dispatch owns start token")
            .claim_for_run();
        let result = match decision {
            ExecutorStartDecision::CompleteCancelled => Err(ExternalFailure::cancelled()),
            ExecutorStartDecision::Run => run_blocking(self.job.take().expect("dispatch owns job")),
        };
        self.job.take();
        self.sink
            .take()
            .expect("dispatch owns sink")
            .deliver(result)
    }
}

impl Drop for BlockingExecutorDispatch {
    fn drop(&mut self) {
        if let Some(sink) = self.sink.take() {
            let _ = sink.deliver(Err(ExternalFailure::cancelled()));
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
    identity: RuntimeIssuedCompletionIdentity,
    sink: Option<CompletionSink>,
    start: Option<ExecutorStartToken>,
    job: Option<AsyncJob>,
}

impl AsyncExecutorDispatch {
    pub fn operation_id(&self) -> OperationId {
        self.identity.operation_id
    }

    pub fn into_future(mut self) -> AsyncDispatchFuture {
        let decision = self
            .start
            .take()
            .expect("dispatch owns start token")
            .claim_for_run();
        let future = match decision {
            ExecutorStartDecision::CompleteCancelled => {
                AsyncFutureState::Immediate(Some(Err(ExternalFailure::cancelled())))
            }
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
    }
}

enum AsyncFutureState {
    Running(AsyncJobFuture),
    Immediate(Option<JobResult>),
    Done,
}

fn construct_async(job: AsyncJob) -> AsyncFutureState {
    #[cfg(panic = "unwind")]
    {
        match catch_unwind(AssertUnwindSafe(job)) {
            Ok(future) => AsyncFutureState::Running(future),
            Err(_) => AsyncFutureState::Immediate(Some(Err(ExternalFailure::worker_panic()))),
        }
    }
    #[cfg(panic = "abort")]
    {
        AsyncFutureState::Running(job())
    }
}

pub struct AsyncDispatchFuture {
    identity: RuntimeIssuedCompletionIdentity,
    sink: Option<CompletionSink>,
    future: AsyncFutureState,
}

impl AsyncDispatchFuture {
    pub fn operation_id(&self) -> OperationId {
        self.identity.operation_id
    }
}

impl Future for AsyncDispatchFuture {
    type Output = CompletionDelivery;

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
            AsyncFutureState::Immediate(result) => {
                result.take().expect("immediate result is polled once")
            }
            AsyncFutureState::Done => panic!("completed dispatch future polled again"),
        };
        self.future = AsyncFutureState::Done;
        let delivery = self.sink.take().expect("future owns sink").deliver(result);
        Poll::Ready(delivery)
    }
}

impl Drop for AsyncDispatchFuture {
    fn drop(&mut self) {
        if let Some(sink) = self.sink.take() {
            let _ = sink.deliver(Err(ExternalFailure::cancelled()));
        }
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
    submission: ExecutorSubmission,
}

impl SubmissionRejected {
    pub fn kind(&self) -> SubmitErrorKind {
        self.kind
    }

    pub fn operation_id(&self) -> OperationId {
        self.submission.operation_id()
    }

    pub fn rollback(self) -> SubmitErrorKind {
        self.kind
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutorAttachError {
    DuplicateRuntime { runtime_id: RuntimeId },
    ShuttingDown,
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
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

    impl CancelHook for LocalHook {
        fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
            self.0.set(self.0.get() + 1);
            Ok(CancelDisposition::Reaped)
        }

        fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
            Ok(CancelDisposition::Reaped)
        }
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
        RuntimeIssuedCompletionIdentity,
    ) {
        let sender = RecordingSender::new(delivery);
        let (_, registrar) = CompletionRegistrar::register(sender.clone()).unwrap();
        let identity = registrar.issue_identity(selected_kind).unwrap();
        let (runtime, submission) = registrar.bind(identity, prepared).unwrap().split();
        (sender, runtime, submission, identity)
    }

    fn blocking(result: JobResult) -> PreparedExternalOperation {
        PreparedExternalOperation::interruptible_blocking(
            kind(91),
            decoder(),
            interruptible(),
            move || result,
        )
    }

    fn poll_once(future: &mut AsyncDispatchFuture) -> Poll<CompletionDelivery> {
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
            let (_sender, runtime, _submission, _) =
                registration(operation, kind(2), CompletionDelivery::Delivered);
            let (_, resource, _) = runtime.into_parts();
            assert!(matches!(
                resource,
                ResourceClass::Interruptible { .. } | ResourceClass::QuarantinedBounded(_)
            ));
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
    fn registrar_identity_and_kind_override_job_declaration() {
        let (sender, _runtime, submission, identity) = registration(
            blocking(Ok(Box::new(1_u8))),
            kind(7),
            CompletionDelivery::Delivered,
        );
        let ExecutorDispatch::Blocking(dispatch) = submission.into_dispatch() else {
            panic!("blocking dispatch expected")
        };
        dispatch.run();
        let completion = sender.take();
        assert_eq!(completion.runtime_id, identity.runtime_id());
        assert_eq!(completion.wait_id, identity.wait_id());
        assert_eq!(completion.generation, identity.generation());
        assert_eq!(completion.operation_id, identity.operation_id());
        assert_eq!(completion.kind, kind(7));
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
        assert_eq!(rejection.operation_id(), identity.operation_id());
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
        assert_eq!(dispatch.run(), CompletionDelivery::Delivered);
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
            blocking(Err(ExternalFailure::new(
                ExternalFailureCode::BoundExceeded,
                "bound",
            ))),
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
        blocking(Err(ExternalFailure::new(
            ExternalFailureCode::WorkerPanic,
            "abort build fixture",
        )))
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
            || async {
                Err(ExternalFailure::new(
                    ExternalFailureCode::DeadlineExceeded,
                    "deadline",
                ))
            },
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
            || {
                std::future::ready(Err(ExternalFailure::new(
                    ExternalFailureCode::WorkerPanic,
                    "abort build fixture",
                )))
            },
        );

        for prepared in [normal, returned_error, panics] {
            let (sender, _runtime, submission, _) =
                registration(prepared, kind(1), CompletionDelivery::Delivered);
            let ExecutorDispatch::Async(dispatch) = submission.into_dispatch() else {
                unreachable!()
            };
            let mut future = dispatch.into_future();
            assert!(matches!(
                poll_once(&mut future),
                Poll::Ready(CompletionDelivery::Delivered)
            ));
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
        assert_eq!(dispatch.run(), CompletionDelivery::InboxClosed);
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
}
