use std::time::Duration;

use crate::cycle::GcEdge;
use crate::{SemaError, Value};

use super::{CancelReason, ChannelId, PreparedExternalOperation, PromiseId, TaskContext, Trace};

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
    pub task_context: &'a mut TaskContext,
    pub cancellation: CancellationView,
}

pub type NativeResult = Result<NativeOutcome, SemaError>;

pub enum NativeOutcome {
    Return(Value),
    Call(NativeCall),
    Suspend(NativeSuspend),
}

pub struct NativeCall {
    pub callable: Value,
    pub args: Vec<Value>,
    pub continuation: Box<dyn NativeContinuation>,
}

pub struct NativeSuspend {
    pub wait: WaitKind,
    pub continuation: Box<dyn NativeContinuation>,
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
        }
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
            Self::Timer(_) | Self::Promise(_) => true,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
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

    fn edge_count(trace: &impl Trace) -> usize {
        let mut count = 0;
        assert!(trace.trace(&mut |_| count += 1));
        count
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
        let mut task_context = TaskContext::default();
        let mut context = NativeCallContext {
            task_context: &mut task_context,
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
    fn native_fn_dual_abi_preserves_legacy_and_runtime_paths() {
        let eval = EvalContext::new();
        let seen_eval = Rc::new(Cell::new(std::ptr::null::<EvalContext>()));
        let mut task_context = TaskContext::default();
        let mut runtime = NativeCallContext {
            task_context: &mut task_context,
            cancellation: CancellationView::default(),
        };
        let legacy = NativeFn::simple("legacy", |_| Ok(Value::int(7)));
        assert_eq!((legacy.func)(&eval, &[]).unwrap(), Value::int(7));
        assert!(
            matches!(legacy.invoke_runtime(&eval, &mut runtime, &[]), Ok(NativeOutcome::Return(v)) if v == Value::int(7))
        );
        let seen_eval_from_callback = Rc::clone(&seen_eval);
        let with_ctx = NativeFn::with_ctx("with-ctx", move |ctx, _| {
            seen_eval_from_callback.set(ctx);
            Ok(Value::int(6))
        });
        assert!(
            matches!(with_ctx.invoke_runtime(&eval, &mut runtime, &[]), Ok(NativeOutcome::Return(v)) if v == Value::int(6))
        );
        assert_eq!(seen_eval.get(), &eval as *const EvalContext);
        let payload: Rc<dyn std::any::Any> = Rc::new(Value::int(5));
        let with_payload = NativeFn::with_payload("with-payload", Rc::clone(&payload), |_, _| {
            Ok(Value::int(5))
        });
        assert!(Rc::ptr_eq(with_payload.payload.as_ref().unwrap(), &payload));
        assert!(
            matches!(with_payload.invoke_runtime(&eval, &mut runtime, &[]), Ok(NativeOutcome::Return(v)) if v == Value::int(5))
        );

        let result =
            NativeFn::simple_result("runtime", |_| Ok(NativeOutcome::Return(Value::int(8))));
        assert!(
            matches!(result.invoke_runtime(&eval, &mut runtime, &[]), Ok(NativeOutcome::Return(v)) if v == Value::int(8))
        );
        assert!((result.func)(&eval, &[])
            .unwrap_err()
            .to_string()
            .contains("runtime"));

        let contextual = NativeFn::with_context_result("contextual", |runtime, args| {
            assert!(!runtime.cancellation.is_requested());
            let _task_context = &mut runtime.task_context;
            Ok(NativeOutcome::Return(args[0].clone()))
        });
        assert!(
            matches!(contextual.invoke_runtime(&eval, &mut runtime, &[Value::int(9)]), Ok(NativeOutcome::Return(v)) if v == Value::int(9))
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
        let mut task_context = TaskContext::default();
        let mut runtime = NativeCallContext {
            task_context: &mut task_context,
            cancellation: CancellationView::default(),
        };
        assert!(matches!(
            native.invoke_runtime(&eval, &mut runtime, &[Value::int(11)]),
            Ok(NativeOutcome::Return(value)) if value == Value::int(10)
        ));
        assert_eq!(*payload.value.borrow(), Value::int(11));
        assert!((native.func)(&eval, &[])
            .unwrap_err()
            .to_string()
            .contains("payload-runtime"));
    }
}
