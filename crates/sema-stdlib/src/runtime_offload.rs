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

use std::rc::Rc;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    downcast_send_payload, CancelDisposition, CancelHook, CancelHookError, CompletionDecoder,
    CompletionKind, DecodedCompletion, ExternalFailure, InterruptibleResource, NativeCallContext,
    NativeContinuation, NativeOutcome, NativeResult, NativeSuspend, PreparedExternalOperation,
    ResourceGateId, ResumeInput, RuntimeRequest, RuntimeResponse, SendPayload, Trace, WaitKind,
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

/// Core assembler for the ASYNC tier: build the interruptible-async External
/// suspend from an already-constructed `resource` (owning the cancel hook) + its
/// `cancel_rx`. The job future runs directly on the executor's async reactor (via
/// [`PreparedExternalOperation::interruptible_async`] → `tokio::spawn`), so it
/// pins no blocking worker while suspended and N concurrent ops overlap. A
/// mid-flight cancel fires the hook's one-shot signal; the `biased` `select!`
/// drops the request future (socket closed / `kill_on_drop` child killed) — the
/// same teardown the blocking tier gives, without an `io_block_on` worker.
fn suspend_with_resource_async<T, F, Fut>(
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

/// Offload an interruptible async I/O op onto the executor's ASYNC tier (real
/// reactor, no per-op blocking worker), mapping the domain `Result<T, String>`
/// to a `Value` on resume via an INFALLIBLE `to_value`. Cancellation drops the
/// in-flight future and tears the resource down. This is the reference async
/// path (http); its blocking-tier sibling is [`external_io_interruptible`].
pub(crate) fn external_io_async<T, F, Fut>(
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
    external_io_async_try(
        op,
        kind,
        resource_label,
        move |value| Ok(to_value(value)),
        make_future,
    )
}

/// Like [`external_io_async`], but the VM-thread `decode` may fail — for ops that
/// inspect the raw result and raise a domain error. Uses the generic
/// drop-on-cancel hook.
pub(crate) fn external_io_async_try<T, F, Fut, D>(
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
    suspend_with_resource_async(op, kind, resource, cancel_rx, Box::new(decode), make_future)
}

/// Offload an interruptible async I/O op onto the executor's blocking tier,
/// running its future via [`sema_io::io_block_on`] and mapping the domain
/// `Result<T, String>` to a `Value` on resume via an INFALLIBLE `to_value`.
/// Cancellation drops the in-flight future and tears the resource down.
///
/// This is the one-call reference path for a cancellable I/O op whose success
/// payload always decodes to a value. Ops whose decode may itself fail (a
/// subprocess non-zero exit) use [`external_io_interruptible_try`]. Retained as
/// the blocking-tier sibling of [`external_io_async`]: ops that cannot run on the
/// async reactor (a synchronous library call under `io_block_on`) pick this;
/// reactor-native ops (http) use the async tier.
#[allow(dead_code)]
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

/// Build a checkout `abort` hook that SIGKILLs the process **group** led by
/// `pid` (Unix). The subprocess modules (`proc`, `pty`, `serial` where a child
/// is spawned) put their child in its own group (`process_group(0)` → pgid ==
/// pid), so the negative pid tears down the leader **and** any grandchildren a
/// compound command (`sh -c "a | b"`) forked — the same teardown shell's
/// killpg gives. Fires only on cancellation of an in-flight checkout op: the op
/// is still parked in the blocking worker holding the `Child`, so `pid` is
/// unreaped and valid when this runs; best-effort past that (a child that
/// exited in the same instant is a no-op or, extremely rarely, a reused pid —
/// the documented `spawn_blocking` cancellation tradeoff). No-op on non-Unix.
pub(crate) fn group_sigkill_abort(pid: u32) -> Box<dyn FnOnce()> {
    Box::new(move || {
        #[cfg(unix)]
        {
            if pid != 0 {
                // SAFETY: a plain signal send to the child's own process group.
                unsafe {
                    libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                }
            }
        }
        #[cfg(not(unix))]
        let _ = pid;
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Checkout-external: gate-guarded offload of a per-handle non-Send-resource op.
// ─────────────────────────────────────────────────────────────────────────────
//
// Six stdlib modules (sqlite, kv, proc, pty, serial, stream) own one resource per
// open handle that at most one offloaded op may hold at a time (a `rusqlite`
// connection, a sled db, a child process, a pty, a serial port, a stream). The
// legacy design open-coded an `Available/CheckedOut/Tombstone` slot + an
// `Acquire`-phase poll loop that re-attempted the checkout on every `AwaitIo`
// poll. `checkout_external` replaces that with two first-class runtime waits:
//
//   1. `WaitKind::ResourceSlot(gate)` — a per-handle [`ResourceGate`] provides
//      FIFO mutual exclusion. A free gate grants immediately; a busy one parks
//      the acquirer FIFO (no polling). `close`/`release` are runtime requests.
//   2. `WaitKind::External` — once the gate is owned, the `Send` resource is
//      taken out of the module's thread-local slot and moved onto the executor's
//      blocking tier with the op; the decoder checks it back in on the VM thread
//      and the continuation releases the gate.
//
// Lifecycle across a single call:
//   * (optional) `Runtime(CreateResourceGate)` if the handle has no gate yet;
//     the returned id is stored via `store_gate` and reused for later ops.
//   * `Suspend(ResourceSlot(gate))` → on grant, `take` the resource from the slot.
//   * `Suspend(External)` → job runs `op(&mut res)` off the VM thread and returns
//     `(res, Result<T, String>)`; the decoder `reinstall`s `res` and decodes `T`.
//   * `Runtime(ReleaseResourceGate)` → wake the FIFO head, then return / raise.
//
// Cancellation:
//   * cancelled while QUEUED behind a busy gate → the runtime's `ResourceSlot`
//     cancel arm removes the waiter from the FIFO; no resource was taken, the
//     gate is untouched, `AcquireCont` just propagates the cancellation.
//   * cancelled mid-op (after the gate is owned) → the External wait's cancel
//     hook `tombstone`s the slot (the resource is stuck in the blocking worker
//     and cannot be reclaimed — best-effort, matching the retired `IoHandle`
//     policy) and runs the optional `abort` (e.g. proc process-group SIGKILL);
//     `ReleaseReturnCont` then releases the gate so queued acquirers wake and
//     fail fast on the tombstone.
//
// KNOWN NARROW RACE (documented, same class as UCR-3 for channel rendezvous):
// a cancellation recorded in the exact window between a `release` granting this
// task the slot and its `AcquireCont` running is delivered as `Cancelled`, which
// `AcquireCont` treats as a queued-cancel (no release) — leaking the gate busy.
// Reaching it requires another task to cancel this one in the same drive tick the
// prior owner releases. C2's eager-cancellation delivery closes this window.
//
// GC: none of the continuations capture a live `Value`/`Env` (the `decode`
// closure builds a `Value` but must not capture one, same rule the file/http
// decoders follow); `FinalCont` alone carries the resolved `Value`/`SemaError`
// across the gate-release round-trip and traces it. Every other continuation and
// the decoder emit zero edges (asserted in the module tests).

/// The blocking checkout op: runs off the VM thread on the executor's blocking
/// tier, mutating the `Send` resource and returning a `Send` payload / error.
type CheckoutJob<Res, T> = Box<dyn FnOnce(&mut Res) -> Result<T, String> + Send>;

/// The VM-thread decode step: turns the op's payload into a `Value` (or a domain
/// error). MUST NOT capture a `Value`/`Env` (it is not traced).
type CheckoutDecode<T> = Box<dyn FnOnce(T) -> Result<Value, SemaError>>;

/// The module-supplied pieces of one checkout offload. `Res` is the `Send`
/// resource; `T` is the op's `Send` result payload.
pub(crate) struct CheckoutOp<Res: Send + 'static, T: Send + 'static> {
    /// Op name for error text (matches the sync path's `op:` prefix).
    pub op_name: &'static str,
    /// Completion kind tag for the External wait.
    pub kind: CompletionKind,
    /// The handle's gate, or `None` to create one first and store it.
    pub gate: Option<ResourceGateId>,
    /// Records a freshly-created gate id against the handle (VM thread).
    pub store_gate: Box<dyn FnOnce(ResourceGateId)>,
    /// Take the resource out of the slot once the gate is owned (VM thread).
    /// Returns a clear domain error for a tombstoned/missing slot.
    pub take: Box<dyn FnOnce() -> Result<Res, SemaError>>,
    /// The blocking op, run off the VM thread on the executor's blocking tier.
    pub op: CheckoutJob<Res, T>,
    /// Reinstall the resource into the slot on completion (VM thread).
    pub reinstall: Box<dyn FnOnce(Res)>,
    /// Decode the op payload into a `Value` (VM thread). MUST NOT capture a
    /// `Value`/`Env` (it is not traced). When `success_value` is `Some`, that
    /// value is returned on op success INSTEAD of calling `decode` (and `decode`
    /// is never invoked) — used by ops that return a caller-supplied `Value`
    /// (e.g. `kv/set` returns the value it stored).
    pub decode: CheckoutDecode<T>,
    /// A caller-supplied `Value` to return on op success, carried as a TRACED
    /// edge across the offload (unlike a `Value` captured in `decode`, which the
    /// GC cannot see). `None` for ops that build their result from the payload.
    pub success_value: Option<Value>,
    /// Mark the slot tombstoned when the resource cannot be reclaimed
    /// (cancel / worker loss). Called at most once (VM thread).
    pub tombstone: Rc<dyn Fn(String)>,
    /// Extra teardown to run on cancel besides the tombstone (e.g. a
    /// process-group SIGKILL). Runs on the VM thread.
    pub abort: Option<Box<dyn FnOnce()>>,
}

/// Entry point: build the gate-acquire suspension (creating the gate first if the
/// handle has none yet). See the module comment for the full lifecycle.
pub(crate) fn checkout_external<Res: Send + 'static, T: Send + 'static>(
    op: CheckoutOp<Res, T>,
) -> NativeResult {
    match op.gate {
        Some(gate) => Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::ResourceSlot(gate),
            continuation: Box::new(AcquireCont { op: Some(op) }),
        })),
        None => Ok(NativeOutcome::Runtime(RuntimeRequest::CreateResourceGate {
            continuation: Box::new(CreateGateCont { op: Some(op) }),
        })),
    }
}

/// Stage 0: a freshly-created gate arrives; store it against the handle, then
/// suspend on it. Holds no `Value`.
struct CreateGateCont<Res: Send + 'static, T: Send + 'static> {
    op: Option<CheckoutOp<Res, T>>,
}

impl<Res: Send + 'static, T: Send + 'static> Trace for CreateGateCont<Res, T> {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl<Res: Send + 'static, T: Send + 'static> NativeContinuation for CreateGateCont<Res, T> {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let mut op = self
            .op
            .expect("checkout gate-create continuation is resumed exactly once");
        match input {
            ResumeInput::Runtime(RuntimeResponse::ResourceGate(gate)) => {
                (op.store_gate)(gate);
                op.store_gate = Box::new(|_| {});
                op.gate = Some(gate);
                Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::ResourceSlot(gate),
                    continuation: Box::new(AcquireCont { op: Some(op) }),
                }))
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(_) => Err(SemaError::eval(format!(
                "{} was cancelled before its resource gate was created",
                op.op_name
            ))),
            ResumeInput::Returned(_) | ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
                "{}: unexpected runtime response creating resource gate",
                op.op_name
            ))),
        }
    }
}

/// Stage 1: the gate slot is granted; check out the resource and offload the op.
/// Holds no `Value`.
struct AcquireCont<Res: Send + 'static, T: Send + 'static> {
    op: Option<CheckoutOp<Res, T>>,
}

impl<Res: Send + 'static, T: Send + 'static> Trace for AcquireCont<Res, T> {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl<Res: Send + 'static, T: Send + 'static> NativeContinuation for AcquireCont<Res, T> {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let op = self
            .op
            .expect("checkout acquire continuation is resumed exactly once");
        let gate = op.gate.expect("gate is known once acquired");
        match input {
            // Slot granted: we now own `gate`.
            ResumeInput::Runtime(RuntimeResponse::Value(_)) => {
                let CheckoutOp {
                    op_name,
                    kind,
                    take,
                    op: job,
                    reinstall,
                    decode,
                    tombstone,
                    abort,
                    success_value,
                    ..
                } = op;
                match take() {
                    Ok(mut resource) => {
                        // Blocking-tier job: run the op off the VM thread and carry
                        // the resource back with the result. A mid-op cancel cannot
                        // interrupt the sync op (best-effort) — the cancel hook
                        // tombstones the slot; the completion is then discarded.
                        let decoder = Box::new(CheckoutDecoder::<Res, T> {
                            op_name,
                            reinstall: Some(reinstall),
                            decode: Some(decode),
                            success_value,
                            tombstone: tombstone.clone(),
                        });
                        let resource_handle = InterruptibleResource::new(
                            op_name,
                            Box::new(CheckoutCancelHook {
                                tombstone,
                                abort,
                                op_name,
                            }),
                        );
                        let prepared = PreparedExternalOperation::interruptible_blocking(
                            kind,
                            decoder,
                            resource_handle,
                            move || {
                                let result = job(&mut resource);
                                Ok(Box::new((resource, result)) as SendPayload)
                            },
                        );
                        Ok(NativeOutcome::Suspend(NativeSuspend {
                            wait: WaitKind::External(Box::new(prepared)),
                            continuation: Box::new(ReleaseReturnCont { op_name, gate }),
                        }))
                    }
                    // Slot is tombstoned/missing: we own the gate, so release it
                    // (waking the next acquirer, who also fails fast) then raise.
                    Err(error) => Ok(NativeOutcome::Runtime(
                        RuntimeRequest::ReleaseResourceGate {
                            gate,
                            continuation: Box::new(FinalCont::Fail(error)),
                        },
                    )),
                }
            }
            // Gate closed while we were queued: never owned it, just raise.
            ResumeInput::Failed(error) => Err(error),
            // Cancelled while queued: the runtime's ResourceSlot cancel arm already
            // removed us from the FIFO; we never owned the gate.
            ResumeInput::Cancelled(_) => Err(SemaError::eval(format!(
                "{} was cancelled while waiting for its resource slot",
                op.op_name
            ))),
            ResumeInput::Returned(_) | ResumeInput::Runtime(_) => {
                Err(SemaError::eval("checkout: unexpected runtime response"))
            }
        }
    }
}

/// The External-wait decoder: reinstall the resource, then decode the payload or
/// render the job's error. On a worker-level failure the resource never came
/// back, so the slot is tombstoned. Holds no `Value`.
struct CheckoutDecoder<Res: Send + 'static, T: Send + 'static> {
    op_name: &'static str,
    reinstall: Option<Box<dyn FnOnce(Res)>>,
    decode: Option<CheckoutDecode<T>>,
    success_value: Option<Value>,
    tombstone: Rc<dyn Fn(String)>,
}

impl<Res: Send + 'static, T: Send + 'static> Trace for CheckoutDecoder<Res, T> {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        // The caller-supplied success value is a live edge; everything else is a
        // plain-data closure that captures no `Value`.
        if let Some(value) = &self.success_value {
            sink(GcEdge::Value(value));
        }
        true
    }
}

impl<Res: Send + 'static, T: Send + 'static> CompletionDecoder for CheckoutDecoder<Res, T> {
    fn decode(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        result: Result<SendPayload, ExternalFailure>,
    ) -> DecodedCompletion {
        let op_name = self.op_name;
        match result {
            Ok(payload) => {
                match downcast_send_payload::<(Res, Result<T, String>)>(payload, op_name) {
                    Ok((resource, op_result)) => {
                        (self.reinstall.take().expect("reinstall once"))(resource);
                        match op_result {
                            Ok(value) => match self.success_value.take() {
                                Some(literal) => Ok(literal),
                                None => (self.decode.take().expect("decode once"))(value),
                            },
                            Err(message) => Err(SemaError::Io(message)),
                        }
                    }
                    Err(failure) => {
                        (self.tombstone)(format!(
                            "{op_name} lost its resource: {}",
                            failure.message()
                        ));
                        Err(SemaError::eval(failure.message().to_string()))
                    }
                }
            }
            Err(failure) => {
                (self.tombstone)(format!("{op_name} worker failed: {}", failure.message()));
                Err(SemaError::eval(format!("{op_name}: {}", failure.message())))
            }
        }
    }
}

/// The External-wait cancel hook: tombstone the slot (the resource is stuck in
/// the blocking worker) and run any extra abort (process-group SIGKILL). Runs on
/// the VM thread; holds no `Value`.
struct CheckoutCancelHook {
    op_name: &'static str,
    tombstone: Rc<dyn Fn(String)>,
    abort: Option<Box<dyn FnOnce()>>,
}

impl Trace for CheckoutCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for CheckoutCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        (self.tombstone)(format!(
            "{} was cancelled while in flight; the resource cannot be reclaimed",
            self.op_name
        ));
        if let Some(abort) = self.abort.take() {
            abort();
        }
        Ok(CancelDisposition::Reaped)
    }
    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(CancelDisposition::Reaped)
    }
}

/// Stage 2: the op completed / failed / was cancelled — release the gate (waking
/// the FIFO head), then deliver the resolved value or raise. Holds no `Value`.
struct ReleaseReturnCont {
    op_name: &'static str,
    gate: ResourceGateId,
}

impl Trace for ReleaseReturnCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for ReleaseReturnCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let final_cont: Box<dyn NativeContinuation> = match input {
            ResumeInput::Returned(value) => Box::new(FinalCont::Value(value)),
            ResumeInput::Failed(error) => Box::new(FinalCont::Fail(error)),
            ResumeInput::Cancelled(_) => Box::new(FinalCont::Cancelled {
                op_name: self.op_name,
            }),
            ResumeInput::Runtime(_) => Box::new(FinalCont::Fail(SemaError::eval(format!(
                "{}: unexpected runtime response after offload",
                self.op_name
            )))),
        };
        Ok(NativeOutcome::Runtime(
            RuntimeRequest::ReleaseResourceGate {
                gate: self.gate,
                continuation: final_cont,
            },
        ))
    }
}

/// Stage 3: the gate is released; deliver the resolved outcome to the caller.
enum FinalCont {
    Value(Value),
    Fail(SemaError),
    Cancelled { op_name: &'static str },
}

impl Trace for FinalCont {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        match self {
            Self::Value(value) => {
                sink(GcEdge::Value(value));
                true
            }
            Self::Fail(error) => error.trace(sink),
            Self::Cancelled { .. } => true,
        }
    }
}

impl NativeContinuation for FinalCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        _input: ResumeInput,
    ) -> NativeResult {
        // The gate release always succeeds; deliver the stored outcome. (If the
        // task carries a sticky cancellation, the runtime settles it Cancelled
        // regardless of what we return here.)
        match *self {
            FinalCont::Value(value) => Ok(NativeOutcome::Return(value)),
            FinalCont::Fail(error) => Err(error),
            FinalCont::Cancelled { op_name } => {
                Err(SemaError::eval(format!("{op_name} was cancelled")))
            }
        }
    }
}

#[cfg(test)]
mod checkout_trace_tests {
    use super::*;

    fn edge_count(trace: &dyn Trace) -> usize {
        let mut count = 0;
        assert!(trace.trace(&mut |_| count += 1));
        count
    }

    /// The checkout continuations that carry NO `Value` must emit zero GC edges
    /// (the CORE-2 rule that keeps continuation state cycle-free). `FinalCont`
    /// alone carries the resolved value/error across the gate-release round-trip
    /// and must trace exactly it. The generic stages hold only boxed `FnOnce`
    /// closures (which must not capture a `Value`) — their trace is edge-free.
    /// (`ReleaseReturnCont` holds only `Copy` fields — an op name + gate id — so
    /// it is edge-free by construction and needs no runtime to mint an id.)
    #[test]
    fn checkout_continuations_trace_expected_edges() {
        assert_eq!(
            edge_count(&CheckoutCancelHook {
                op_name: "t",
                tombstone: Rc::new(|_| {}),
                abort: None,
            }),
            0
        );
        // FinalCont: the sole Value-carrying continuation.
        assert_eq!(edge_count(&FinalCont::Value(Value::int(7))), 1);
        assert_eq!(edge_count(&FinalCont::Fail(SemaError::eval("boom"))), 0);
        assert_eq!(edge_count(&FinalCont::Cancelled { op_name: "t" }), 0);
        // A UserException error carries a Value edge that must be traced.
        assert_eq!(
            edge_count(&FinalCont::Fail(SemaError::UserException(Value::int(3)))),
            1
        );
        // The generic acquire/create stages and the decoder hold no Value.
        let decoder: CheckoutDecoder<(), ()> = CheckoutDecoder {
            op_name: "t",
            reinstall: Some(Box::new(|_| {})),
            decode: Some(Box::new(|_| Ok(Value::nil()))),
            success_value: None,
            tombstone: Rc::new(|_| {}),
        };
        assert_eq!(edge_count(&decoder), 0);
        // A carried success value is a traced edge (kv/set's return value).
        let decoder_with_value: CheckoutDecoder<(), ()> = CheckoutDecoder {
            op_name: "t",
            reinstall: Some(Box::new(|_| {})),
            decode: Some(Box::new(|_| Ok(Value::nil()))),
            success_value: Some(Value::int(9)),
            tombstone: Rc::new(|_| {}),
        };
        assert_eq!(edge_count(&decoder_with_value), 1);
    }
}
