//! Shared glue for offloading an interruptible I/O op onto the unified-runtime
//! executor and surfacing it structurally as a `NativeOutcome::Suspend`.
//!
//! This is the REFERENCE shape every External-wait I/O subsystem reuses (http is
//! the first; git and shell follow). An op splits into three pieces that respect
//! the send/non-send boundary:
//!
//! * a `Send` **job** that runs off the VM thread on the executor's worker pool
//!   and produces a plain `Result<T, String>` — `Ok(T)` (a send-safe payload) or
//!   a job-level error message. It never touches a `Value`/`Rc`.
//! * a **decoder** that runs back on the VM thread and turns the send payload
//!   into a `Value` — the only place a `Value` may be built. It may itself fail
//!   (e.g. a subprocess non-zero exit → a domain error).
//! * a **continuation** that resumes the parked frame with the decoded value, or
//!   raises the error / a cancellation at the call site.
//!
//! ## Why the BLOCKING tier (not `interruptible_async`)
//!
//! The obvious fit is [`PreparedExternalOperation::interruptible_async`], whose
//! ABI models a tokio future run off the VM thread with drop-on-cancel. But the
//! shipping `ThreadPoolExecutor` (sema-vm `runtime/host.rs`) drives async
//! dispatches with a bare thread-parking `block_on` and NO tokio reactor
//! ("sema-vm carries no async runtime") — so a `reqwest`/`tokio::process` future
//! panics there ("there is no reactor running"). The sanctioned way to run such a
//! future off the VM thread is [`sema_io::io_block_on`] on the executor's (plain
//! OS thread) blocking worker, which the sema-io blocking tier is explicitly
//! built for. We therefore run the future via
//! [`PreparedExternalOperation::interruptible_blocking`] + `io_block_on`, and
//! preserve the retired `IoHandle::with_abort` teardown by racing the work
//! against a cancel signal in a `tokio::select!`: on `async/cancel`/`async/timeout`
//! the [`CancelHook`] fires the signal, the select drops the in-flight future
//! (closing the socket / dropping a `kill_on_drop` child), and the resource is
//! torn down. An op that needs an EXTRA synchronous teardown (shell's
//! process-group `SIGKILL`) supplies its own [`CancelHook`] via
//! [`suspend_external_interruptible_try`].

use std::future::Future;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    downcast_send_payload, CancelDisposition, CancelHook, CancelHookError, CompletionDecoder,
    CompletionKind, DecodedCompletion, ExternalFailure, InterruptibleResource, NativeCallContext,
    NativeContinuation, NativeOutcome, NativeResult, NativeSuspend, PreparedExternalOperation,
    ResumeInput, SendPayload, Trace, WaitKind,
};
use sema_core::{SemaError, Value};

/// The executor-facing job result: a send payload or a worker-level failure
/// (cancellation / panic / bound-exceeded). Spelled out here because the alias
/// in the executor is private.
type JobResult = Result<SendPayload, ExternalFailure>;

/// A VM-thread decode step: consumes the offloaded job's `Send` payload `T` and
/// builds the final `Value`, or raises a domain error (e.g. a subprocess
/// non-zero exit). Runs on the VM thread, so it may build `Value`s freely — but
/// it MUST NOT CAPTURE a live `Value`/`Env` (the decoder is not traced), the same
/// rule the file/http decoders follow.
type DecodeFn<T> = Box<dyn FnOnce(T) -> Result<Value, SemaError>>;

/// Decodes an offloaded job's send payload back into a `Value` on the VM thread.
/// The payload is a `Result<T, String>`: `Ok(T)` is handed to the caller's
/// `decode` (which may itself fail); `Err(message)` is a job-level I/O error
/// rendered as `SemaError::Io` (matching the synchronous path). A worker-level
/// [`ExternalFailure`] (panic / bound-exceeded) surfaces as an evaluation error
/// tagged with the op name. (A genuine cancellation is settled by the runtime as
/// `ResumeInput::Cancelled` and never reaches this decoder.)
struct IoDecoder<T: Send + 'static> {
    op: &'static str,
    decode: DecodeFn<T>,
}

impl<T: Send + 'static> Trace for IoDecoder<T> {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        // Holds no live `Value`/`Env` (the decode closure captures only plain
        // data — op name, arg strings) — nothing to trace.
        true
    }
}

impl<T: Send + 'static> CompletionDecoder for IoDecoder<T> {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: JobResult,
    ) -> DecodedCompletion {
        match result {
            Ok(payload) => match downcast_send_payload::<Result<T, String>>(payload, self.op) {
                Ok(Ok(value)) => (self.decode)(value),
                Ok(Err(message)) => Err(SemaError::Io(message)),
                Err(failure) => Err(SemaError::eval(failure.message().to_string())),
            },
            Err(failure) => Err(SemaError::eval(format!(
                "{}: {}",
                self.op,
                failure.message()
            ))),
        }
    }
}

/// Resumes the parked frame once the offloaded job completes: the decoded value
/// is injected onto the stack top; a failure or cancellation is raised at the
/// call site (catchable by an enclosing try/catch, and by `async/timeout`).
struct IoOffloadContinuation {
    op: &'static str,
}

impl Trace for IoOffloadContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for IoOffloadContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "{} was cancelled ({reason:?})",
                self.op
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
                "{} continuation received an unexpected runtime response",
                self.op
            ))),
        }
    }
}

/// Cancel hook for an interruptible I/O op whose in-flight future is torn down by
/// firing a one-shot signal that a `tokio::select!` in the job awaits. Firing the
/// signal makes the select drop the request future (closing the socket / dropping
/// a `kill_on_drop` child) — the retired `IoHandle::with_abort` teardown. Lives on
/// the runtime thread (never crosses to a worker), so it need not be `Send`.
struct SelectCancelHook {
    signal: Option<CancelSignal>,
}

impl Trace for SelectCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for SelectCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        if let Some(signal) = self.signal.take() {
            // Err (receiver already gone) means the job finished first; nothing
            // to tear down. Either way the resource is reaped.
            let _ = signal.send(());
        }
        Ok(CancelDisposition::Reaped)
    }
    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

/// A cancel signal paired to a job's `select!`: the [`CancelHook`] holds the
/// sender, the job holds the receiver. Callers that need a bespoke hook (e.g.
/// shell's process-group kill) build their hook around the sender and pass the
/// receiver to [`suspend_external_interruptible_try`].
pub(crate) type CancelSignal = tokio::sync::oneshot::Sender<()>;

/// The receiver half handed to the offloaded job.
pub(crate) type CancelWaiter = tokio::sync::oneshot::Receiver<()>;

/// Make a fresh cancel-signal pair.
pub(crate) fn cancel_channel() -> (CancelSignal, CancelWaiter) {
    tokio::sync::oneshot::channel()
}

/// Core assembler: build the interruptible-blocking External suspend from an
/// already-constructed `resource` (owning the cancel hook) + its `cancel_rx`.
fn suspend_with_resource<T, F, Fut>(
    op: &'static str,
    kind: CompletionKind,
    resource: InterruptibleResource,
    cancel_rx: CancelWaiter,
    decode: DecodeFn<T>,
    make_future: F,
) -> NativeResult
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, String>> + Send + 'static,
{
    let decoder = Box::new(IoDecoder { op, decode });
    let continuation = Box::new(IoOffloadContinuation { op });
    let prepared =
        PreparedExternalOperation::interruptible_blocking(kind, decoder, resource, move || {
            // On a plain executor worker thread `io_block_on` is legal and enters
            // the shared io runtime, giving the future its reactor. The `biased`
            // select checks the cancel signal first so a cancel that raced ahead
            // of dispatch skips the work entirely; otherwise a mid-flight cancel
            // drops the future here (socket closed / `kill_on_drop` child killed).
            let out: Result<T, String> = sema_io::io_block_on(async move {
                tokio::select! {
                    biased;
                    _ = cancel_rx => Err("cancelled".to_string()),
                    result = make_future() => result,
                }
            });
            Ok(Box::new(out) as SendPayload)
        });
    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation,
    }))
}

/// Offload an interruptible async I/O op onto the executor's blocking tier,
/// running its future via [`sema_io::io_block_on`] and mapping the domain
/// `Result<T, String>` to a `Value` on resume via an INFALLIBLE `to_value`.
/// Cancellation drops the in-flight future and tears the resource down.
///
/// This is the one-call reference path for a cancellable I/O op whose success
/// payload always decodes to a value (e.g. http). Ops whose decode may itself
/// fail (a subprocess non-zero exit) use [`external_io_interruptible_try`].
pub(crate) fn external_io_interruptible<T, F, Fut>(
    op: &'static str,
    kind: CompletionKind,
    resource_label: &'static str,
    to_value: fn(T) -> Value,
    make_future: F,
) -> NativeResult
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, String>> + Send + 'static,
{
    external_io_interruptible_try(
        op,
        kind,
        resource_label,
        move |value| Ok(to_value(value)),
        make_future,
    )
}

/// Like [`external_io_interruptible`], but the VM-thread `decode` may fail — for
/// ops that inspect the raw result and raise a domain error (a subprocess
/// non-zero exit, a parse failure). Uses the generic drop-on-cancel hook.
pub(crate) fn external_io_interruptible_try<T, F, Fut, D>(
    op: &'static str,
    kind: CompletionKind,
    resource_label: &'static str,
    decode: D,
    make_future: F,
) -> NativeResult
where
    T: Send + 'static,
    D: FnOnce(T) -> Result<Value, SemaError> + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, String>> + Send + 'static,
{
    let (cancel_tx, cancel_rx) = cancel_channel();
    let resource = InterruptibleResource::new(
        resource_label,
        Box::new(SelectCancelHook {
            signal: Some(cancel_tx),
        }),
    );
    suspend_with_resource(op, kind, resource, cancel_rx, Box::new(decode), make_future)
}

/// Like [`external_io_interruptible_try`], but the caller supplies the `resource`
/// (owning a BESPOKE cancel hook built around the `cancel_tx` from
/// [`cancel_channel`]) and its `cancel_rx`. Used by ops whose cancellation needs
/// more than dropping the future — e.g. shell's synchronous process-group
/// `SIGKILL` for a `sh -c` pipeline's grandchildren.
pub(crate) fn suspend_external_interruptible_try<T, F, Fut, D>(
    op: &'static str,
    kind: CompletionKind,
    resource: InterruptibleResource,
    cancel_rx: CancelWaiter,
    decode: D,
    make_future: F,
) -> NativeResult
where
    T: Send + 'static,
    D: FnOnce(T) -> Result<Value, SemaError> + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, String>> + Send + 'static,
{
    suspend_with_resource(op, kind, resource, cancel_rx, Box::new(decode), make_future)
}
