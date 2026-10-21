//! Host-registered async native functions: the adapter behind
//! `sema::Interpreter::register_async_fn`
//! (`docs/plans/2026-10-21-async-native-registration.md`).
//!
//! A host async function splits into three pieces that respect the
//! send/non-send boundary, exactly like the stdlib async offload path
//! (`sema-stdlib`'s `runtime_offload`, which delegates its cancel hook and
//! assembler to this module so both paths share one cancellation mechanism):
//!
//! * **prepare** runs on the interpreter thread and turns `&[Value]` into an
//!   owned `Send` input;
//! * **run** executes on THE process-wide tokio pool and awaits a `Send` future
//!   over that input, producing an owned `Send` output or a host error;
//! * **finish** runs back on the interpreter thread and builds the `Value`.
//!
//! # Cancellation (D4)
//!
//! The process executor drops the tokio abort handle for async dispatches
//! (`executor.rs`): cancellation reaches the job only through the wait's
//! [`CancelHook`] on the interpreter thread. [`SelectCancelHook`] owns a
//! one-shot sender; the job races the host future against the receiver in a
//! `biased` `tokio::select!`, so firing the signal drops the host future at its
//! next poll and the task self-completes. Late completions are discarded by the
//! runtime's wait generation. A host future stuck in a blocking call, or a task
//! it detached with `tokio::spawn`, is not stopped.
//!
//! # Error mapping (D2)
//!
//! A host error is rendered to text (its `Display` chain) on the executor and
//! raised as `SemaError::Io("<name>: <text>")` on the interpreter thread. Each
//! [`ExternalFailureCode`] keeps a distinct prefix (see [`failure_error`]) so
//! cancellation, executor rejection, deadline, worker panic and decode failures
//! stay distinguishable from an ordinary host error in `catch`.

use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    downcast_send_payload, CancelDisposition, CancelHook, CancelHookError, CompletionDecoder,
    CompletionKind, DecodedCompletion, ExternalFailure, ExternalFailureCode, InterruptibleResource,
    NativeCallContext, NativeContinuation, NativeOutcome, NativeResult, NativeSuspend,
    PreparedExternalOperation, ResumeInput, SendPayload, Trace, WaitKind,
};
use sema_core::{SemaError, Value};

/// The one completion kind shared by every host-registered async function
/// (D3). The kind is only checked for self-consistency when a completion is
/// delivered, so per-registration kinds would buy nothing.
pub const HOST_ASYNC_COMPLETION_KIND: u64 = 0x686f_7374; // "host"

/// Resource-class label for host async operations in executor accounting.
pub const HOST_ASYNC_RESOURCE: &str = "host-async";

/// The executor-facing job result: a send payload or a worker-level failure.
type JobResult = Result<SendPayload, ExternalFailure>;

/// The sender half of a cancel signal; owned by the wait's [`SelectCancelHook`].
pub type CancelSignal = tokio::sync::oneshot::Sender<()>;

/// The receiver half of a cancel signal; handed to the offloaded job.
pub type CancelWaiter = tokio::sync::oneshot::Receiver<()>;

/// Make a fresh cancel-signal pair.
pub fn cancel_channel() -> (CancelSignal, CancelWaiter) {
    tokio::sync::oneshot::channel()
}

/// Cancel hook for an interruptible async op whose in-flight future is torn
/// down by firing a one-shot signal that a `tokio::select!` in the job awaits.
/// Lives on the interpreter thread (never crosses to a worker), so it need not
/// be `Send`. Holds no `Value`/`Env`, so it traces no edges.
pub struct SelectCancelHook {
    signal: Option<CancelSignal>,
}

impl SelectCancelHook {
    pub fn new(signal: CancelSignal) -> Self {
        Self {
            signal: Some(signal),
        }
    }
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

/// An interruptible resource whose cancel hook fires a fresh cancel signal,
/// paired with the receiver the job must `select!` against.
pub fn cancellable_resource(kind: &'static str) -> (InterruptibleResource, CancelWaiter) {
    let (cancel_tx, cancel_rx) = cancel_channel();
    let resource = InterruptibleResource::new(kind, Box::new(SelectCancelHook::new(cancel_tx)));
    (resource, cancel_rx)
}

/// Assemble an interruptible-async External suspend: the job future runs on the
/// executor's async tier (via [`PreparedExternalOperation::interruptible_async`]
/// → `io_spawn`), pins no blocking worker while suspended, and is dropped when
/// `cancel_rx` fires. `make_future` is invoked inside the spawned task, so the
/// host future is constructed in tokio runtime context. The payload delivered
/// to `decoder` is a boxed `Result<T, String>`.
pub fn suspend_async<T, F, Fut>(
    kind: CompletionKind,
    resource: InterruptibleResource,
    cancel_rx: CancelWaiter,
    decoder: Box<dyn CompletionDecoder>,
    continuation: Box<dyn NativeContinuation>,
    make_future: F,
) -> NativeResult
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, String>> + Send + 'static,
{
    let prepared = PreparedExternalOperation::interruptible_async(
        kind,
        decoder,
        resource,
        move || async move {
            let out: Result<T, String> = tokio::select! {
                biased;
                _ = cancel_rx => Err("cancelled".to_string()),
                result = make_future() => result,
            };
            Ok(Box::new(out) as SendPayload)
        },
    );
    Ok(NativeOutcome::Suspend(NativeSuspend {
        wait: WaitKind::External(Box::new(prepared)),
        continuation,
    }))
}

/// Render a host error and its `source()` chain as one line of text on the
/// executor, so no error object crosses back to the interpreter thread.
pub fn render_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Map a worker-level failure to a `SemaError` whose message keeps the failure
/// code distinguishable from an ordinary host error (D2).
pub fn failure_error(name: &str, failure: &ExternalFailure) -> SemaError {
    let prefix = match failure.code() {
        ExternalFailureCode::Rejected => "executor rejected the operation",
        ExternalFailureCode::Cancelled => "operation cancelled",
        ExternalFailureCode::DeadlineExceeded => "deadline exceeded",
        ExternalFailureCode::BoundExceeded => "bound exceeded",
        ExternalFailureCode::WorkerPanic => "worker panicked",
        ExternalFailureCode::Decode => "payload decode failed",
    };
    SemaError::eval(format!("{name}: {prefix}: {}", failure.message()))
}

/// The interpreter-thread `finish` step, shared by every call of one
/// registration. `Rc`, not `Arc`: it never leaves the interpreter thread.
pub type FinishFn<T> = Rc<dyn Fn(T) -> Result<Value, SemaError>>;

/// Decodes a host async job's payload on the interpreter thread. Retains the
/// function name and the shared `finish` closure only; neither may hold a
/// `Value`/`Env` (Invariant I2), so it traces no edges.
pub struct HostAsyncDecoder<T: Send + 'static> {
    name: Arc<str>,
    finish: FinishFn<T>,
}

impl<T: Send + 'static> Trace for HostAsyncDecoder<T> {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl<T: Send + 'static> CompletionDecoder for HostAsyncDecoder<T> {
    fn decode(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: JobResult,
    ) -> DecodedCompletion {
        match result {
            Ok(payload) => {
                match downcast_send_payload::<Result<T, String>>(payload, "register_async_fn") {
                    Ok(Ok(output)) => (self.finish)(output),
                    Ok(Err(text)) => Err(SemaError::Io(format!("{}: {text}", self.name))),
                    Err(failure) => Err(failure_error(&self.name, &failure)),
                }
            }
            Err(failure) => Err(failure_error(&self.name, &failure)),
        }
    }
}

/// Resumes the parked frame once the host job completes: the decoded value is
/// returned; a failure is raised at the call site (catchable by `try`); a
/// cancellation settled by the runtime is raised with the function name.
/// Retains the function name only, so it traces no edges.
pub struct HostAsyncContinuation {
    name: Arc<str>,
}

impl Trace for HostAsyncContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for HostAsyncContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "{}: operation cancelled ({reason:?})",
                self.name
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
                "{}: continuation received an unexpected runtime response",
                self.name
            ))),
        }
    }
}

/// Suspend the calling Sema task on a host async function call: `run(input)`
/// is awaited on the process-wide tokio pool, and `finish` converts its output
/// on the interpreter thread. `prepare` has already run; this is the step after
/// it. The bounds mirror `Interpreter::register_async_fn`.
pub fn suspend_host_async<Input, Output, Error, Run, Fut, Finish>(
    name: &Arc<str>,
    input: Input,
    run: &Arc<Run>,
    finish: &Rc<Finish>,
) -> NativeResult
where
    Input: Send + 'static,
    Output: Send + 'static,
    Error: std::error::Error + Send + 'static,
    Run: Fn(Input) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Output, Error>> + Send + 'static,
    Finish: Fn(Output) -> Result<Value, SemaError> + 'static,
{
    let kind = CompletionKind::try_from_raw(HOST_ASYNC_COMPLETION_KIND)
        .expect("host async completion kind is nonzero");
    let (resource, cancel_rx) = cancellable_resource(HOST_ASYNC_RESOURCE);
    let finish: FinishFn<Output> =
        Rc::clone(finish) as Rc<dyn Fn(Output) -> Result<Value, SemaError>>;
    let decoder = Box::new(HostAsyncDecoder {
        name: Arc::clone(name),
        finish,
    });
    let continuation = Box::new(HostAsyncContinuation {
        name: Arc::clone(name),
    });
    let run = Arc::clone(run);
    suspend_async(
        kind,
        resource,
        cancel_rx,
        decoder,
        continuation,
        move || async move { run(input).await.map_err(|error| render_error_chain(&error)) },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Outer(Inner);
    #[derive(Debug)]
    struct Inner;

    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("outer failed")
        }
    }
    impl std::fmt::Display for Inner {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("inner cause")
        }
    }
    impl std::error::Error for Inner {}
    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn error_chain_is_rendered_outer_to_inner() {
        assert_eq!(
            render_error_chain(&Outer(Inner)),
            "outer failed: inner cause"
        );
    }

    #[test]
    fn failure_codes_keep_distinct_prefixes() {
        let rejected = failure_error("host/x", &ExternalFailure::rejected());
        assert!(rejected
            .to_string()
            .contains("host/x: executor rejected the operation"));
        let deadline = failure_error("host/x", &ExternalFailure::deadline_exceeded("late"));
        assert!(deadline
            .to_string()
            .contains("host/x: deadline exceeded: late"));
    }

    #[test]
    fn cancel_hook_fires_signal_once_and_reaps() {
        let (tx, mut rx) = cancel_channel();
        let mut hook = SelectCancelHook::new(tx);
        assert_eq!(hook.cancel().unwrap(), CancelDisposition::Reaped);
        assert!(rx.try_recv().is_ok());
        assert_eq!(hook.cancel().unwrap(), CancelDisposition::Reaped);
        assert_eq!(hook.reap().unwrap(), CancelDisposition::Reaped);
    }
}
