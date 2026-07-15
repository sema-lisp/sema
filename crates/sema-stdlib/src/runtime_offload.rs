//! Shared glue for offloading an interruptible I/O op onto the unified-runtime
//! executor and surfacing it structurally as a `NativeOutcome::Suspend`.
//!
//! This is the REFERENCE shape every External-wait I/O subsystem reuses (http is
//! the first; git/sqlite/kv/proc/ws/pty/serial/stream follow). An op splits into
//! three pieces that respect the send/non-send boundary:
//!
//! * a `Send` **job** that runs off the VM thread on the executor's worker pool
//!   and produces a plain `Result<T, String>` — `Ok(T)` (a send-safe payload) or
//!   a domain I/O error message. It never touches a `Value`/`Rc`.
//! * a **decoder** that runs back on the VM thread and turns the send payload
//!   into a `Value` (this is the only place a `Value` may be built).
//! * a **continuation** that resumes the parked frame with the decoded value, or
//!   raises the error / a cancellation at the call site.
//!
//! ## Why the BLOCKING tier (not `interruptible_async`)
//!
//! The obvious fit is [`PreparedExternalOperation::interruptible_async`], whose
//! ABI models a tokio future run off the VM thread with drop-on-cancel. But the
//! shipping `ThreadPoolExecutor` (sema-vm `runtime/host.rs`) drives async
//! dispatches with a bare thread-parking `block_on` and NO tokio reactor
//! ("sema-vm carries no async runtime") — so a `reqwest` future panics there
//! ("there is no reactor running"). The sanctioned way to run a `reqwest` future
//! off the VM thread is [`sema_io::io_block_on`] on the executor's (plain OS
//! thread) blocking worker, which the sema-io blocking tier is explicitly built
//! for. We therefore run the future via [`PreparedExternalOperation::interruptible_blocking`]
//! + `io_block_on`, and preserve the retired `IoHandle::with_abort` teardown by
//! racing the request against a cancel signal in a `tokio::select!`: on
//! `async/cancel`/`async/timeout` the [`CancelHook`] fires the signal, the select
//! drops the in-flight request future, and the connection is torn down (no wasted
//! round-trip). See the F2 report for the follow-up that would let this move back
//! to `interruptible_async` (teach the executor's async tier to spawn onto the
//! shared io runtime with drop-on-cancel).

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

/// Decodes an offloaded job's send payload back into a `Value` on the VM thread.
/// The payload is a domain `Result<T, String>`: `Ok(T)` maps through `to_value`;
/// `Err(message)` is a domain I/O error rendered as `SemaError::Io` (identical to
/// the synchronous path). A worker-level [`ExternalFailure`] (panic / bound-
/// exceeded) surfaces as an evaluation error tagged with the op name. (A genuine
/// cancellation is settled by the runtime as `ResumeInput::Cancelled` and never
/// reaches this decoder.)
pub(crate) struct IoOffloadDecoder<T: Send + 'static> {
    op: &'static str,
    to_value: fn(T) -> Value,
}

impl<T: Send + 'static> Trace for IoOffloadDecoder<T> {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl<T: Send + 'static> CompletionDecoder for IoOffloadDecoder<T> {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: JobResult,
    ) -> DecodedCompletion {
        match result {
            Ok(payload) => match downcast_send_payload::<Result<T, String>>(payload, self.op) {
                Ok(Ok(value)) => Ok((self.to_value)(value)),
                Ok(Err(message)) => Err(SemaError::Io(message)),
                Err(failure) => Err(SemaError::eval(failure.message().to_string())),
            },
            Err(failure) => Err(SemaError::eval(format!("{}: {}", self.op, failure.message()))),
        }
    }
}

/// Resumes the parked frame once the offloaded job completes: the decoded value
/// is injected onto the stack top; a failure or cancellation is raised at the
/// call site (catchable by an enclosing try/catch, and by `async/timeout`).
pub(crate) struct IoOffloadContinuation {
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
/// signal makes the select drop the request future (closing the socket) — the
/// exact teardown the retired `IoHandle::with_abort` performed. Lives on the
/// runtime thread (never crosses to a worker), so it need not be `Send`.
struct SelectCancelHook {
    signal: Option<tokio::sync::oneshot::Sender<()>>,
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

/// Offload an interruptible async I/O op onto the executor's blocking tier,
/// running its future via [`sema_io::io_block_on`] and decoding the domain
/// `Result<T, String>` to a `Value` on resume. Cancellation (`async/cancel` /
/// `async/timeout`) drops the in-flight future and tears the connection down.
///
/// This is the one-call reference path for a cancellable I/O op. `make_future`
/// is a `Send` future factory (built lazily on the worker under the io reactor):
///
/// ```ignore
/// external_io_interruptible("http", kind, "http", raw_to_value,
///     move || async move { do_request(builder).await /* -> Result<Raw, String> */ })
/// ```
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
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let resource = InterruptibleResource::new(
        resource_label,
        Box::new(SelectCancelHook {
            signal: Some(cancel_tx),
        }),
    );
    let decoder = Box::new(IoOffloadDecoder { op, to_value });
    let continuation = Box::new(IoOffloadContinuation { op });
    let prepared = PreparedExternalOperation::interruptible_blocking(kind, decoder, resource, move || {
        // On a plain executor worker thread `io_block_on` is legal and enters the
        // shared io runtime, giving the request future its reactor. The `biased`
        // select checks the cancel signal first so a cancel that raced ahead of
        // dispatch skips the request entirely; otherwise a mid-flight cancel drops
        // the request future here (connection torn down).
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
