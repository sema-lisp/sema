use std::cell::Cell;
use std::fmt;
use std::rc::Rc;
use std::time::Duration;

use crate::cycle::GcEdge;
use crate::{Env, EvalContext, SemaError, Value};

use super::{
    CancelReason, ChannelId, PreparedExternalOperation, PromiseId, ResourceGateId,
    TaskContextHandle, TaskOutcome, TaskSettlement, Trace,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CancellationView {
    requested: bool,
    reason: Option<CancelReason>,
}

impl CancellationView {
    #[doc(hidden)]
    pub fn new(requested: bool, reason: Option<CancelReason>) -> Self {
        Self { requested, reason }
    }

    pub fn is_requested(&self) -> bool {
        self.requested
    }

    pub fn reason(&self) -> Option<&CancelReason> {
        self.reason.as_ref()
    }
}

pub struct NativeCallContext<'a> {
    pub eval_context: &'a EvalContext,
    pub task_context: TaskContextHandle,
    pub call_env: Option<Rc<Env>>,
    pub cancellation: CancellationView,
}

pub type NativeResult = Result<NativeOutcome, SemaError>;

pub enum NativeOutcome {
    Return(Value),
    Call(NativeCall),
    Suspend(NativeSuspend),
    Runtime(RuntimeRequest),
}

pub enum RuntimeRequest {
    Spawn {
        callable: Value,
        continuation: Box<dyn NativeContinuation>,
    },
    CancelPromise {
        promise: PromiseId,
        continuation: Box<dyn NativeContinuation>,
    },
    CreateChannel {
        capacity: usize,
        continuation: Box<dyn NativeContinuation>,
    },
    ChannelOp {
        channel: ChannelId,
        operation: ChannelOperation,
        continuation: Box<dyn NativeContinuation>,
    },
    CreateSettledPromise {
        outcome: TaskOutcome,
        continuation: Box<dyn NativeContinuation>,
    },
    InspectPromise {
        promise: PromiseId,
        continuation: Box<dyn NativeContinuation>,
    },
    PromiseSetWait {
        wait: PromiseSetWait,
        continuation: Box<dyn NativeContinuation>,
    },
    OriginBarrier {
        continuation: Box<dyn NativeContinuation>,
    },
    /// Allocate a fresh [`ResourceGateId`] — a per-handle mutual-exclusion slot
    /// with a FIFO waiter queue. A checkout-style stdlib module (sqlite, kv,
    /// proc, pty, serial, stream) creates one gate per resource handle when the
    /// handle is opened, then acquires it via [`WaitKind::ResourceSlot`] before
    /// each offloaded op and releases it via [`RuntimeRequest::ReleaseResourceGate`]
    /// when the op completes. The continuation receives [`RuntimeResponse::ResourceGate`].
    CreateResourceGate {
        continuation: Box<dyn NativeContinuation>,
    },
    /// Release ownership of a previously-acquired resource gate, waking the FIFO
    /// head waiter (if any) so exactly one queued acquirer proceeds. The
    /// continuation resumes with `RuntimeResponse::Value(nil)`.
    ReleaseResourceGate {
        gate: ResourceGateId,
        continuation: Box<dyn NativeContinuation>,
    },
    /// Close a resource gate: fail every parked waiter with a structured
    /// "gate closed" error and drop the gate record. Used when a handle is
    /// closed/tombstoned so queued acquirers fail fast rather than hang.
    CloseResourceGate {
        gate: ResourceGateId,
        continuation: Box<dyn NativeContinuation>,
    },
}

pub enum ChannelOperation {
    Close,
    TryReceive,
    Inspect(ChannelQuery),
}
#[derive(Clone, Copy, Debug)]
pub enum ChannelQuery {
    Closed,
    Count,
    Empty,
    Full,
}

pub enum PromiseSetMode {
    All,
    Race,
    Timeout(Duration),
}
pub struct PromiseSetWait {
    pub promises: Vec<PromiseId>,
    pub mode: PromiseSetMode,
}

/// A VM-thread capability for one runtime resource gate.
///
/// Native checkout paths use [`ResourceGateHandle::id`] with the ordinary
/// `WaitKind::ResourceSlot` / `RuntimeRequest` protocol. The close capability
/// exists for lifecycle edges that cannot first store the id (allocation
/// delivery cancelled) and host-only cleanup that runs outside a native
/// continuation. Clones share the close-once state.
#[derive(Clone)]
pub struct ResourceGateHandle {
    id: ResourceGateId,
    closed: Rc<Cell<bool>>,
    closer: Rc<ResourceGateCloser>,
}

type ResourceGateCloser = dyn Fn(ResourceGateId) -> Result<bool, ResourceGateCloseError> + 'static;

impl ResourceGateHandle {
    /// Construct a gate capability around the owning runtime's weak closer.
    /// Runtime implementations are the intended callers.
    #[doc(hidden)]
    pub fn new(id: ResourceGateId, closer: Rc<ResourceGateCloser>) -> Self {
        Self {
            id,
            closed: Rc::new(Cell::new(false)),
            closer,
        }
    }

    pub fn id(&self) -> ResourceGateId {
        self.id
    }

    /// Close the gate through its owning runtime. Returns `Ok(true)` when this
    /// call removed the live gate, `Ok(false)` when it was already closed, and
    /// leaves the capability retryable when runtime coordination fails.
    pub fn close(&self) -> Result<bool, ResourceGateCloseError> {
        if self.closed.get() {
            return Ok(false);
        }
        let removed = (self.closer)(self.id)?;
        self.closed.set(true);
        Ok(removed)
    }
}

impl fmt::Debug for ResourceGateHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResourceGateHandle")
            .field("id", &self.id)
            .field("closed", &self.closed.get())
            .finish_non_exhaustive()
    }
}

impl Trace for ResourceGateHandle {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceGateCloseError {
    RuntimeUnavailable,
    RuntimeBusy,
    WrongRuntime,
}

impl fmt::Display for ResourceGateCloseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeUnavailable => f.write_str("resource gate runtime is no longer available"),
            Self::RuntimeBusy => f.write_str("resource gate runtime is already mutably borrowed"),
            Self::WrongRuntime => f.write_str("resource gate belongs to a different runtime"),
        }
    }
}

impl std::error::Error for ResourceGateCloseError {}

#[derive(Clone, Debug)]
pub enum RuntimeResponse {
    Promise(PromiseId),
    Channel(ChannelId),
    ResourceGate(ResourceGateHandle),
    Value(Value),
    Cancelled(bool),
    Settlement(Option<Rc<TaskSettlement>>),
    Settlements(Vec<Rc<TaskSettlement>>),
    Receive(ChannelReceive),
    Send(ChannelSend),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelSend {
    Sent,
    Closed,
}

#[derive(Clone, Debug)]
pub enum ChannelReceive {
    Received(Value),
    Empty,
    Closed,
}

pub struct NativeCall {
    pub callable: Value,
    pub args: Vec<Value>,
    pub continuation: Box<dyn NativeContinuation>,
}

/// Build the first stage of a structural multimethod call.
///
/// The returned call invokes the dispatch function. Its continuation retains
/// the multimethod, original arguments, and caller continuation, selects a
/// handler from the returned dispatch value, and emits a second
/// [`NativeOutcome::Call`] for that handler. No evaluator callback is performed
/// inside the active runtime quantum.
pub fn multimethod_call(
    multimethod: Value,
    args: Vec<Value>,
    continuation: Box<dyn NativeContinuation>,
) -> Result<NativeCall, SemaError> {
    let dispatch_fn = multimethod
        .as_multimethod_rc()
        .ok_or_else(|| SemaError::type_error("multimethod", multimethod.type_name()))?
        .dispatch_fn
        .clone();
    Ok(NativeCall {
        callable: dispatch_fn,
        args: args.clone(),
        continuation: Box::new(MultimethodDispatchContinuation {
            multimethod,
            args,
            continuation,
        }),
    })
}

struct MultimethodDispatchContinuation {
    multimethod: Value,
    args: Vec<Value>,
    continuation: Box<dyn NativeContinuation>,
}

impl Trace for MultimethodDispatchContinuation {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.multimethod));
        for arg in &self.args {
            sink(GcEdge::Value(arg));
        }
        self.continuation.trace(sink)
    }
}

impl NativeContinuation for MultimethodDispatchContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let dispatch_value = match input {
            ResumeInput::Returned(value) => value,
            ResumeInput::Failed(error) => return Err(error),
            ResumeInput::Cancelled(reason) => {
                return Err(SemaError::eval(format!(
                    "multimethod dispatch was cancelled ({reason:?})"
                )))
            }
            ResumeInput::Runtime(_) => {
                return Err(SemaError::eval(
                    "multimethod dispatch received an unexpected runtime response",
                ))
            }
        };
        let multimethod = self.multimethod.as_multimethod_rc().ok_or_else(|| {
            SemaError::eval("internal error: multimethod dispatch state lost its callable")
        })?;
        let handler = crate::select_multimethod_handler(&multimethod, &dispatch_value)?;
        Ok(NativeOutcome::Call(NativeCall {
            callable: handler,
            args: self.args,
            continuation: self.continuation,
        }))
    }
}

pub struct NativeSuspend {
    pub wait: WaitKind,
    pub continuation: Box<dyn NativeContinuation>,
}

pub enum WaitKind {
    Timer(Duration),
    Promise(PromiseId),
    PromiseSet(PromiseSetWait),
    Channel(ChannelWait),
    External(Box<PreparedExternalOperation>),
    /// Park until this task owns `gate`'s exclusive slot. Resumes with
    /// `RuntimeResponse::Value(nil)` once the slot is granted (immediately if
    /// the gate is free, otherwise FIFO-behind any earlier acquirers).
    ResourceSlot(ResourceGateId),
}

pub enum ChannelWait {
    Send { channel: ChannelId, value: Value },
    Receive { channel: ChannelId },
}

pub enum ResumeInput {
    Returned(Value),
    Failed(SemaError),
    Cancelled(CancelReason),
    Runtime(RuntimeResponse),
}

pub trait NativeContinuation: Trace {
    fn resume(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult;
}

fn trace_error(error: &SemaError, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
    match error {
        SemaError::UserException(value) | SemaError::Condition(value) => {
            sink(GcEdge::Value(value));
            true
        }
        SemaError::WithTrace { inner, .. } | SemaError::WithContext { inner, .. } => {
            trace_error(inner, sink)
        }
        _ => true,
    }
}

impl Trace for NativeOutcome {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        match self {
            Self::Return(value) => {
                sink(GcEdge::Value(value));
                true
            }
            Self::Call(call) => call.trace(sink),
            Self::Suspend(suspend) => suspend.trace(sink),
            Self::Runtime(request) => request.trace(sink),
        }
    }
}

impl Trace for RuntimeRequest {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        match self {
            Self::Spawn {
                callable,
                continuation,
            } => {
                sink(GcEdge::Value(callable));
                continuation.trace(sink)
            }
            Self::CreateSettledPromise {
                outcome,
                continuation,
            } => outcome.trace(sink) && continuation.trace(sink),
            Self::CancelPromise { continuation, .. }
            | Self::CreateChannel { continuation, .. }
            | Self::ChannelOp { continuation, .. }
            | Self::InspectPromise { continuation, .. }
            | Self::PromiseSetWait { continuation, .. }
            | Self::CreateResourceGate { continuation }
            | Self::ReleaseResourceGate { continuation, .. }
            | Self::CloseResourceGate { continuation, .. } => continuation.trace(sink),
            Self::OriginBarrier { continuation } => continuation.trace(sink),
        }
    }
}

impl Trace for RuntimeResponse {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        match self {
            Self::Value(value) => sink(GcEdge::Value(value)),
            Self::Receive(ChannelReceive::Received(value)) => sink(GcEdge::Value(value)),
            Self::Settlement(Some(settlement)) => return settlement.trace(sink),
            Self::Settlements(settlements) => {
                return settlements.iter().all(|settlement| settlement.trace(sink));
            }
            _ => {}
        }
        true
    }
}

impl Trace for NativeCall {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.callable));
        for arg in &self.args {
            sink(GcEdge::Value(arg));
        }
        self.continuation.trace(sink)
    }
}

impl Trace for NativeSuspend {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        self.wait.trace(sink) && self.continuation.trace(sink)
    }
}

impl Trace for WaitKind {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        match self {
            Self::Timer(_) | Self::Promise(_) | Self::PromiseSet(_) | Self::ResourceSlot(_) => true,
            Self::Channel(wait) => wait.trace(sink),
            Self::External(operation) => operation.trace(sink),
        }
    }
}

impl Trace for ChannelWait {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        match self {
            Self::Send { value, .. } => sink(GcEdge::Value(value)),
            Self::Receive { .. } => {}
        }
        true
    }
}

impl Trace for ResumeInput {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        match self {
            Self::Returned(value) => {
                sink(GcEdge::Value(value));
                true
            }
            Self::Failed(error) => trace_error(error, sink),
            Self::Cancelled(_) => true,
            Self::Runtime(response) => response.trace(sink),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::time::Duration;

    use crate::cycle::GcEdge;
    use crate::{EvalContext, NativeFn, SemaError, Value};

    use super::*;
    use crate::runtime::{ChannelId, RuntimeId, RuntimeScopedIdCounter, Trace};

    struct Continuation {
        edge: Value,
        seen: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Trace for Continuation {
        fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            sink(GcEdge::Value(&self.edge));
            true
        }
    }

    impl NativeContinuation for Continuation {
        fn resume(
            self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            input: ResumeInput,
        ) -> NativeResult {
            self.seen.borrow_mut().push(match input {
                ResumeInput::Returned(_) => "returned",
                ResumeInput::Failed(_) => "failed",
                ResumeInput::Cancelled(_) => "cancelled",
                ResumeInput::Runtime(_) => "runtime",
            });
            Ok(NativeOutcome::Return(self.edge))
        }
    }

    fn channel() -> ChannelId {
        let runtime = RuntimeId::allocate().unwrap();
        RuntimeScopedIdCounter::new(runtime).allocate().unwrap()
    }

    fn promise() -> PromiseId {
        let runtime = RuntimeId::allocate().unwrap();
        RuntimeScopedIdCounter::new(runtime).allocate().unwrap()
    }

    fn resource_gate() -> ResourceGateId {
        let runtime = RuntimeId::allocate().unwrap();
        RuntimeScopedIdCounter::new(runtime).allocate().unwrap()
    }

    fn edge_count(trace: &impl Trace) -> usize {
        let mut count = 0;
        assert!(trace.trace(&mut |_| count += 1));
        count
    }

    fn test_multimethod() -> Value {
        let mut methods = BTreeMap::new();
        methods.insert(Value::keyword("selected"), Value::string("handler"));
        Value::multimethod(crate::MultiMethod {
            name: crate::intern("test/structural-multimethod"),
            dispatch_fn: Value::string("dispatch"),
            methods: RefCell::new(methods),
            default: RefCell::new(None),
        })
    }

    #[test]
    fn resource_gate_handle_is_trace_trivial_and_closes_once_across_clones() {
        let gate = resource_gate();
        let calls = Rc::new(Cell::new(0));
        let calls_for_close = Rc::clone(&calls);
        let handle = ResourceGateHandle::new(
            gate,
            Rc::new(move |_| {
                calls_for_close.set(calls_for_close.get() + 1);
                Ok(true)
            }),
        );
        let clone = handle.clone();

        assert_eq!(handle.id(), gate);
        assert_eq!(edge_count(&handle), 0);
        assert_eq!(handle.close(), Ok(true));
        assert_eq!(clone.close(), Ok(false));
        assert_eq!(calls.get(), 1, "the underlying closer runs exactly once");
    }

    #[test]
    fn resource_gate_handle_remains_retryable_after_coordination_failure() {
        let calls = Rc::new(Cell::new(0));
        let calls_for_close = Rc::clone(&calls);
        let handle = ResourceGateHandle::new(
            resource_gate(),
            Rc::new(move |_| {
                calls_for_close.set(calls_for_close.get() + 1);
                if calls_for_close.get() == 1 {
                    Err(ResourceGateCloseError::RuntimeBusy)
                } else {
                    Ok(true)
                }
            }),
        );

        assert_eq!(handle.close(), Err(ResourceGateCloseError::RuntimeBusy));
        assert_eq!(handle.close(), Ok(true));
        assert_eq!(handle.close(), Ok(false));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn dropping_resource_gate_handle_clones_does_not_close_the_gate() {
        let calls = Rc::new(Cell::new(0));
        let calls_for_close = Rc::clone(&calls);
        let handle = ResourceGateHandle::new(
            resource_gate(),
            Rc::new(move |_| {
                calls_for_close.set(calls_for_close.get() + 1);
                Ok(true)
            }),
        );

        drop(handle.clone());
        assert_eq!(
            calls.get(),
            0,
            "temporary capability clones are inert on drop"
        );
        assert_eq!(handle.close(), Ok(true));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn protocol_shapes_and_structural_trace_multiplicity() {
        let value = Value::string("same");
        assert_eq!(edge_count(&NativeOutcome::Return(value.clone())), 1);

        let call = NativeCall {
            callable: value.clone(),
            args: vec![value.clone(), value.clone()],
            continuation: Box::new(Continuation {
                edge: value.clone(),
                seen: Rc::default(),
            }),
        };
        assert_eq!(edge_count(&call), 4);
        assert_eq!(edge_count(&NativeOutcome::Call(call)), 4);

        let send = ChannelWait::Send {
            channel: channel(),
            value: value.clone(),
        };
        assert_eq!(edge_count(&send), 1);
        assert_eq!(edge_count(&ChannelWait::Receive { channel: channel() }), 0);
        assert_eq!(edge_count(&WaitKind::Timer(Duration::from_millis(1))), 0);
        assert_eq!(edge_count(&WaitKind::Promise(promise())), 0);
        assert_eq!(edge_count(&WaitKind::Channel(send)), 1);

        let suspend = NativeSuspend {
            wait: WaitKind::Channel(ChannelWait::Send {
                channel: channel(),
                value: value.clone(),
            }),
            continuation: Box::new(Continuation {
                edge: value.clone(),
                seen: Rc::default(),
            }),
        };
        assert_eq!(edge_count(&suspend), 2);
        assert_eq!(edge_count(&NativeOutcome::Suspend(suspend)), 2);
        assert_eq!(edge_count(&ResumeInput::Returned(value.clone())), 1);
        assert_eq!(
            edge_count(&ResumeInput::Failed(SemaError::Condition(value))),
            1
        );
        assert_eq!(
            edge_count(&ResumeInput::Cancelled(CancelReason::Explicit)),
            0
        );
    }

    #[test]
    fn multimethod_call_traces_both_structural_stages() {
        let retained = Value::string("outer-retained");
        let seen = Rc::new(RefCell::new(Vec::new()));
        let dispatch = multimethod_call(
            test_multimethod(),
            vec![Value::int(7)],
            Box::new(Continuation {
                edge: retained,
                seen,
            }),
        )
        .expect("multimethod call");

        assert_eq!(
            edge_count(&dispatch),
            5,
            "dispatch callable + arg + retained multimethod + retained arg + outer continuation"
        );

        let eval_context = EvalContext::new();
        let task_context = TaskContextHandle::default();
        let mut context = NativeCallContext {
            eval_context: &eval_context,
            task_context,
            call_env: None,
            cancellation: CancellationView::default(),
        };
        let outcome = dispatch
            .continuation
            .resume(
                &mut context,
                ResumeInput::Returned(Value::keyword("selected")),
            )
            .expect("dispatch selects handler");
        let NativeOutcome::Call(handler) = outcome else {
            panic!("dispatch continuation must emit the handler call")
        };
        assert_eq!(handler.callable, Value::string("handler"));
        assert_eq!(handler.args, vec![Value::int(7)]);
        assert_eq!(
            edge_count(&handler),
            3,
            "selected handler + arg + outer continuation"
        );
    }

    #[test]
    fn multimethod_dispatch_propagates_failure_cancellation_and_protocol_error() {
        fn dispatch_continuation() -> Box<dyn NativeContinuation> {
            multimethod_call(
                test_multimethod(),
                vec![Value::int(7)],
                Box::new(Continuation {
                    edge: Value::nil(),
                    seen: Rc::default(),
                }),
            )
            .expect("multimethod call")
            .continuation
        }

        let eval_context = EvalContext::new();
        let task_context = TaskContextHandle::default();
        let mut context = NativeCallContext {
            eval_context: &eval_context,
            task_context,
            call_env: None,
            cancellation: CancellationView::default(),
        };

        let failure = dispatch_continuation()
            .resume(
                &mut context,
                ResumeInput::Failed(SemaError::eval("dispatch failed")),
            )
            .err()
            .expect("failure propagates");
        assert!(failure.to_string().contains("dispatch failed"));

        let cancelled = dispatch_continuation()
            .resume(&mut context, ResumeInput::Cancelled(CancelReason::Explicit))
            .err()
            .expect("cancellation propagates");
        assert!(cancelled.to_string().contains("cancelled"));

        let protocol = dispatch_continuation()
            .resume(
                &mut context,
                ResumeInput::Runtime(RuntimeResponse::Value(Value::nil())),
            )
            .err()
            .expect("unexpected runtime response fails");
        assert!(protocol.to_string().contains("unexpected runtime response"));
    }

    #[test]
    fn tracing_keeps_partial_output_when_continuation_fails() {
        struct BorrowingContinuation {
            first: Value,
            second: Rc<RefCell<Value>>,
        }
        impl Trace for BorrowingContinuation {
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
        impl NativeContinuation for BorrowingContinuation {
            fn resume(
                self: Box<Self>,
                _context: &mut NativeCallContext<'_>,
                _input: ResumeInput,
            ) -> NativeResult {
                Ok(NativeOutcome::Return(self.first))
            }
        }

        let continuation = BorrowingContinuation {
            first: Value::NIL,
            second: Rc::new(RefCell::new(Value::NIL)),
        };
        let second = Rc::clone(&continuation.second);
        let borrow = second.borrow_mut();
        let call = NativeCall {
            callable: Value::NIL,
            args: vec![Value::NIL],
            continuation: Box::new(continuation),
        };
        let mut emitted = 0;
        assert!(!call.trace(&mut |_| emitted += 1));
        assert_eq!(emitted, 3);
        drop(borrow);
    }

    #[test]
    fn continuation_is_consumed_for_each_resume_input() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let eval_context = EvalContext::new();
        let task_context = TaskContextHandle::default();
        let mut context = NativeCallContext {
            eval_context: &eval_context,
            task_context,
            call_env: None,
            cancellation: CancellationView::default(),
        };
        for input in [
            ResumeInput::Returned(Value::NIL),
            ResumeInput::Failed(SemaError::eval("failed")),
            ResumeInput::Cancelled(CancelReason::Explicit),
        ] {
            Box::new(Continuation {
                edge: Value::NIL,
                seen: Rc::clone(&seen),
            })
            .resume(&mut context, input)
            .unwrap();
        }
        assert_eq!(&*seen.borrow(), &["returned", "failed", "cancelled"]);
    }

    #[test]
    fn legacy_runtime_fallback_uses_embedded_eval_context() {
        let embedded = EvalContext::new();
        let observed = Rc::new(Cell::new(std::ptr::null::<EvalContext>()));
        let observed_by_native = Rc::clone(&observed);
        let native = NativeFn::with_ctx("context-identity", move |context, _| {
            observed_by_native.set(context);
            Ok(Value::NIL)
        });
        let task_context = TaskContextHandle::default();
        let mut runtime_context = NativeCallContext {
            eval_context: &embedded,
            task_context,
            call_env: None,
            cancellation: CancellationView::default(),
        };

        assert!(matches!(
            native.invoke_runtime(&mut runtime_context, &[]),
            Ok(NativeOutcome::Return(value)) if value.is_nil()
        ));
        assert_eq!(observed.get(), &embedded as *const EvalContext);
    }

    #[test]
    fn native_fn_dual_abi_preserves_legacy_and_runtime_paths() {
        let eval = EvalContext::new();
        let seen_eval = Rc::new(Cell::new(std::ptr::null::<EvalContext>()));
        let task_context = TaskContextHandle::default();
        let mut runtime = NativeCallContext {
            eval_context: &eval,
            task_context,
            call_env: None,
            cancellation: CancellationView::default(),
        };
        let legacy = NativeFn::simple("legacy", |_| Ok(Value::int(7)));
        assert_eq!((legacy.func)(&eval, &[]).unwrap(), Value::int(7));
        assert!(
            matches!(legacy.invoke_runtime(&mut runtime, &[]), Ok(NativeOutcome::Return(v)) if v == Value::int(7))
        );
        let seen_eval_from_callback = Rc::clone(&seen_eval);
        let with_ctx = NativeFn::with_ctx("with-ctx", move |ctx, _| {
            seen_eval_from_callback.set(ctx);
            Ok(Value::int(6))
        });
        assert!(
            matches!(with_ctx.invoke_runtime(&mut runtime, &[]), Ok(NativeOutcome::Return(v)) if v == Value::int(6))
        );
        assert_eq!(seen_eval.get(), &eval as *const EvalContext);
        let payload: Rc<dyn std::any::Any> = Rc::new(Value::int(5));
        let with_payload = NativeFn::with_payload("with-payload", Rc::clone(&payload), |_, _| {
            Ok(Value::int(5))
        });
        assert!(Rc::ptr_eq(with_payload.payload.as_ref().unwrap(), &payload));
        assert!(
            matches!(with_payload.invoke_runtime(&mut runtime, &[]), Ok(NativeOutcome::Return(v)) if v == Value::int(5))
        );

        let result =
            NativeFn::simple_result("runtime", |_| Ok(NativeOutcome::Return(Value::int(8))));
        assert!(
            matches!(result.invoke_runtime(&mut runtime, &[]), Ok(NativeOutcome::Return(v)) if v == Value::int(8))
        );
        assert!((result.func)(&eval, &[])
            .unwrap_err()
            .to_string()
            .contains("runtime"));

        let contextual = NativeFn::with_context_result("contextual", |runtime, args| {
            assert!(!runtime.cancellation.is_requested());
            let _task_context = runtime.task_context.borrow_mut();
            Ok(NativeOutcome::Return(args[0].clone()))
        });
        assert!(
            matches!(contextual.invoke_runtime(&mut runtime, &[Value::int(9)]), Ok(NativeOutcome::Return(v)) if v == Value::int(9))
        );
        assert!((contextual.func)(&eval, &[])
            .unwrap_err()
            .to_string()
            .contains("contextual"));
    }

    #[test]
    fn payload_runtime_native_uses_one_typed_payload_owner() {
        struct Payload {
            value: RefCell<Value>,
        }

        fn invoke(
            payload: &Payload,
            runtime: &mut NativeCallContext<'_>,
            args: &[Value],
        ) -> NativeResult {
            assert!(!runtime.cancellation.is_requested());
            let previous = payload.value.replace(args[0].clone());
            Ok(NativeOutcome::Return(previous))
        }

        let payload = Rc::new(Payload {
            value: RefCell::new(Value::int(10)),
        });
        let native = NativeFn::with_payload_result("payload-runtime", Rc::clone(&payload), invoke);
        assert_eq!(Rc::strong_count(&payload), 2, "caller plus payload field");
        assert!(native
            .payload
            .as_ref()
            .unwrap()
            .downcast_ref::<Payload>()
            .is_some());

        let eval = EvalContext::new();
        let task_context = TaskContextHandle::default();
        let mut runtime = NativeCallContext {
            eval_context: &eval,
            task_context,
            call_env: None,
            cancellation: CancellationView::default(),
        };
        assert!(matches!(
            native.invoke_runtime(&mut runtime, &[Value::int(11)]),
            Ok(NativeOutcome::Return(value)) if value == Value::int(10)
        ));
        assert_eq!(*payload.value.borrow(), Value::int(11));
        assert!((native.func)(&eval, &[])
            .unwrap_err()
            .to_string()
            .contains("payload-runtime"));
    }
}
