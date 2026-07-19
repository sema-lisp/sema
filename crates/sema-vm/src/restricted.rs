use std::num::NonZeroUsize;
use std::rc::Rc;

use sema_core::runtime::{
    multimethod_call, CancellationView, NativeCall, NativeCallContext, NativeContinuation,
    NativeOutcome, NativeResult, ResumeInput, TaskContextHandle,
};
use sema_core::{Env, EvalContext, SemaError, Value};
use web_time::Instant;

use crate::debug::VmExecResult;
use crate::vm::{snapshot_escaping_call_with_owner, snapshot_native_escaping_args_with_owner};
use crate::{extract_vm_closure, CompiledProgram, VM};

/// Hard limits and diagnostics for one scheduler-free VM evaluation.
#[derive(Clone, Debug)]
pub struct RestrictedRunPolicy {
    pub operation: &'static str,
    pub suspension_error: &'static str,
    pub instruction_limit: NonZeroUsize,
    pub transition_limit: NonZeroUsize,
    pub deadline: Option<Instant>,
    pub cancellation: CancellationView,
}

/// Evaluate a compiled program without driving the runtime scheduler.
///
/// Runtime-ABI calls and continuations are driven inline. Any wait or runtime
/// request is rejected before it can install scheduler state or admit an
/// external job.
pub fn run_program_restricted(
    ctx: &EvalContext,
    task_context: TaskContextHandle,
    program: CompiledProgram,
    globals: Rc<Env>,
    policy: RestrictedRunPolicy,
) -> Result<Value, SemaError> {
    let _task_context = ctx.scope_task_context(task_context.clone());
    let _quantum = if ctx.runtime_quantum_active() {
        None
    } else {
        Some(ctx.enter_runtime_quantum()?)
    };
    RestrictedVmDriver::new(ctx, task_context, policy).run(program, globals)
}

struct RestrictedVmDriver<'a> {
    ctx: &'a EvalContext,
    task_context: TaskContextHandle,
    policy: RestrictedRunPolicy,
    instructions_remaining: usize,
    transitions_remaining: usize,
}

enum RestrictedWork {
    RunVm {
        vm: Box<VM>,
        owner: RestrictedOwner,
    },
    Apply {
        owner: RestrictedOwner,
        result: NativeResult,
    },
    Settle {
        owner: RestrictedOwner,
        result: Result<Value, SemaError>,
    },
}

#[derive(Default)]
struct RestrictedOwner {
    // A hard limit can discard tens of thousands of pending calls at once.
    // Flat storage makes that teardown iterative instead of recursively
    // dropping a boxed owner chain.
    frames: Vec<RestrictedFrame>,
    // The top parked VM supplies call_env and owns escaping callback values.
    // Cache its frame index so a deep continuation-only chain stays O(1) per
    // structural call rather than reverse-scanning the whole frame stack.
    vm_frame_indices: Vec<usize>,
}

enum RestrictedFrame {
    Continuation(Box<dyn NativeContinuation>),
    VmResume(Box<VM>),
}

impl RestrictedOwner {
    fn call_env(&self) -> Option<Rc<Env>> {
        let index = *self.vm_frame_indices.last()?;
        let RestrictedFrame::VmResume(vm) = &self.frames[index] else {
            unreachable!("restricted VM frame index must point to a VM frame")
        };
        Some(vm.active_globals())
    }

    fn parked_parent_vm_mut(&mut self) -> Option<&mut VM> {
        let index = *self.vm_frame_indices.last()?;
        let RestrictedFrame::VmResume(vm) = &mut self.frames[index] else {
            unreachable!("restricted VM frame index must point to a VM frame")
        };
        Some(vm)
    }

    fn push_continuation(&mut self, continuation: Box<dyn NativeContinuation>) {
        self.frames
            .push(RestrictedFrame::Continuation(continuation));
    }

    fn push_vm(&mut self, vm: Box<VM>) {
        self.vm_frame_indices.push(self.frames.len());
        self.frames.push(RestrictedFrame::VmResume(vm));
    }

    fn pop(&mut self) -> Option<RestrictedFrame> {
        let frame = self.frames.pop()?;
        if matches!(frame, RestrictedFrame::VmResume(_)) {
            debug_assert_eq!(self.vm_frame_indices.pop(), Some(self.frames.len()));
        }
        Some(frame)
    }
}

impl<'a> RestrictedVmDriver<'a> {
    fn new(
        ctx: &'a EvalContext,
        task_context: TaskContextHandle,
        policy: RestrictedRunPolicy,
    ) -> Self {
        let instructions_remaining = policy.instruction_limit.get();
        let transitions_remaining = policy.transition_limit.get();
        Self {
            ctx,
            task_context,
            policy,
            instructions_remaining,
            transitions_remaining,
        }
    }

    fn run(mut self, program: CompiledProgram, globals: Rc<Env>) -> Result<Value, SemaError> {
        let mut vm = VM::new(
            globals,
            program.functions,
            &program.native_table,
            program.main_cache_slots,
        )?;
        vm.seed_main_frame(program.closure);
        let mut work = RestrictedWork::RunVm {
            vm: Box::new(vm),
            owner: RestrictedOwner::default(),
        };

        loop {
            work = match work {
                RestrictedWork::RunVm { mut vm, owner } => {
                    self.check_boundary()?;
                    if self.instructions_remaining == 0 {
                        return Err(self.instruction_limit_error());
                    }
                    let quantum = vm.run_quantum(
                        self.ctx,
                        self.instructions_remaining,
                        self.policy.cancellation.clone(),
                    );
                    self.instructions_remaining = self
                        .instructions_remaining
                        .saturating_sub(quantum.instructions);
                    match quantum.outcome {
                        Ok(VmExecResult::Finished(value)) => RestrictedWork::Settle {
                            owner,
                            result: Ok(value),
                        },
                        Err(error) => RestrictedWork::Settle {
                            owner,
                            result: Err(error),
                        },
                        Ok(VmExecResult::Pending(pending)) => {
                            let mut owner = owner;
                            owner.push_vm(vm);
                            RestrictedWork::Apply {
                                owner,
                                result: Ok(pending.into_outcome()),
                            }
                        }
                        Ok(VmExecResult::QuantumExpired { .. }) => {
                            return Err(self.instruction_limit_error())
                        }
                        Ok(VmExecResult::Stopped(_) | VmExecResult::Yielded) => {
                            return Err(SemaError::eval(format!(
                                "{} stopped unexpectedly",
                                self.policy.operation
                            )))
                        }
                    }
                }
                RestrictedWork::Apply { owner, result } => {
                    self.check_boundary()?;
                    match result {
                        Ok(NativeOutcome::Return(value)) => RestrictedWork::Settle {
                            owner,
                            result: Ok(value),
                        },
                        Err(error) => RestrictedWork::Settle {
                            owner,
                            result: Err(error),
                        },
                        Ok(NativeOutcome::Call(call)) => {
                            self.consume_transition()?;
                            self.invoke_call(owner, call)
                        }
                        Ok(NativeOutcome::Suspend(_) | NativeOutcome::Runtime(_)) => {
                            RestrictedWork::Settle {
                                owner,
                                result: Err(SemaError::eval(self.policy.suspension_error)),
                            }
                        }
                    }
                }
                RestrictedWork::Settle { mut owner, result } => {
                    self.check_boundary()?;
                    match owner.pop() {
                        None => return result,
                        Some(RestrictedFrame::Continuation(continuation)) => {
                            self.consume_transition()?;
                            let mut native_context = NativeCallContext {
                                eval_context: self.ctx,
                                task_context: self.task_context.clone(),
                                call_env: owner.call_env(),
                                cancellation: self.policy.cancellation.clone(),
                            };
                            let input = match result {
                                Ok(value) => ResumeInput::Returned(value),
                                Err(error) => ResumeInput::Failed(error),
                            };
                            RestrictedWork::Apply {
                                owner,
                                result: continuation.resume(&mut native_context, input),
                            }
                        }
                        Some(RestrictedFrame::VmResume(mut vm)) => {
                            vm.sync_tracked_upvalues_to_stack();
                            match result {
                                Ok(value) => vm.replace_stack_top(value),
                                Err(error) => vm.resume_with_error(error),
                            }
                            RestrictedWork::RunVm { vm, owner }
                        }
                    }
                }
            };
        }
    }

    fn invoke_call(&self, mut owner: RestrictedOwner, call: NativeCall) -> RestrictedWork {
        if call.callable.as_multimethod_rc().is_some() {
            return RestrictedWork::Apply {
                owner,
                result: multimethod_call(call.callable, call.args, call.continuation)
                    .map(NativeOutcome::Call),
            };
        }

        let NativeCall {
            callable,
            mut args,
            continuation,
        } = call;
        let call_env = owner.call_env();

        if let Some((closure, functions, native_fns)) = extract_vm_closure(&callable) {
            if let Some(parent_vm) = owner.parked_parent_vm_mut() {
                snapshot_escaping_call_with_owner(parent_vm, &callable, &args);
            }
            owner.push_continuation(continuation);
            let Some(globals) = closure.globals.clone() else {
                return RestrictedWork::Settle {
                    owner,
                    result: Err(SemaError::eval("VM closure has no home environment")),
                };
            };
            let mut vm = VM::new_for_task_with_native_fns(globals, functions, native_fns);
            return match vm.setup_for_call_owned(closure, &mut args) {
                Ok(()) => RestrictedWork::RunVm {
                    vm: Box::new(vm),
                    owner,
                },
                Err(error) => RestrictedWork::Settle {
                    owner,
                    result: Err(error),
                },
            };
        }

        if let Some(native) = callable.as_native_fn_rc() {
            if !native.escaping_args().is_empty() {
                if let Some(parent_vm) = owner.parked_parent_vm_mut() {
                    snapshot_native_escaping_args_with_owner(parent_vm, &native, &args);
                }
            }
            let mut native_context = NativeCallContext {
                eval_context: self.ctx,
                task_context: self.task_context.clone(),
                call_env,
                cancellation: self.policy.cancellation.clone(),
            };
            owner.push_continuation(continuation);
            return RestrictedWork::Apply {
                owner,
                result: native.invoke_runtime(&mut native_context, &args),
            };
        }

        let result = if let Some(keyword) = callable.as_keyword_spur() {
            if args.len() != 1 {
                Err(SemaError::arity(
                    sema_core::resolve(keyword),
                    "1",
                    args.len(),
                ))
            } else {
                let key = Value::keyword_from_spur(keyword);
                let arg = &args[0];
                if let Some(map) = arg.as_map_rc() {
                    Ok(map.get(&key).cloned().unwrap_or_else(Value::nil))
                } else if let Some(map) = arg.as_hashmap_rc() {
                    Ok(map.get(&key).cloned().unwrap_or_else(Value::nil))
                } else {
                    Err(SemaError::type_error("map or hashmap", arg.type_name()))
                }
            }
        } else {
            Err(SemaError::type_error("callable", callable.type_name()))
        };
        owner.push_continuation(continuation);
        RestrictedWork::Settle { owner, result }
    }

    fn check_boundary(&self) -> Result<(), SemaError> {
        if self.policy.cancellation.is_requested() {
            return Err(SemaError::eval(format!(
                "{} was cancelled",
                self.policy.operation
            )));
        }
        if self
            .policy
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(SemaError::eval(format!(
                "{} exceeded deadline",
                self.policy.operation
            )));
        }
        Ok(())
    }

    fn consume_transition(&mut self) -> Result<(), SemaError> {
        if self.transitions_remaining == 0 {
            return Err(SemaError::eval(format!(
                "{} exceeded transition limit",
                self.policy.operation
            )));
        }
        self.transitions_remaining -= 1;
        Ok(())
    }

    fn instruction_limit_error(&self) -> SemaError {
        SemaError::eval(format!(
            "{} exceeded instruction limit",
            self.policy.operation
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::num::NonZeroUsize;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use sema_core::cycle::GcEdge;
    use sema_core::runtime::{
        CancelReason, CancellationView, CompletionDecoder, CompletionKind, ExternalFailure,
        NativeCall, NativeCallContext, NativeContinuation, NativeOutcome, NativeResult,
        NativeSuspend, PreparedExternalOperation, QuarantineBound, ResumeInput, RuntimeRequest,
        SendPayload, TaskContextHandle, TaskLocalValue, Trace, WaitKind,
    };
    use sema_core::{Env, EvalContext, MultiMethod, NativeFn, SemaError, Value};
    use web_time::Instant;

    use super::{run_program_restricted, RestrictedRunPolicy};
    use crate::compile_program;

    fn policy() -> RestrictedRunPolicy {
        RestrictedRunPolicy {
            operation: "test evaluation",
            suspension_error: "test evaluation cannot suspend",
            instruction_limit: NonZeroUsize::new(10_000).unwrap(),
            transition_limit: NonZeroUsize::new(100).unwrap(),
            deadline: None,
            cancellation: CancellationView::default(),
        }
    }

    #[test]
    fn restricted_driver_runs_a_pure_program_through_runtime_quantum() {
        let globals = Rc::new(Env::new());
        let forms = sema_reader::read_many("42").expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");
        let ctx = EvalContext::new();

        let value = run_program_restricted(
            &ctx,
            TaskContextHandle::default(),
            program,
            globals,
            policy(),
        )
        .expect("restricted evaluation succeeds");

        assert_eq!(value, Value::int(42));
        assert!(!ctx.runtime_quantum_active());
    }

    #[test]
    fn restricted_driver_preserves_an_ambient_runtime_quantum_guard() {
        let forms = sema_reader::read_many("42").expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");
        let ctx = EvalContext::new();
        let outer_quantum = ctx.enter_runtime_quantum().expect("outer quantum enters");

        let value = run_program_restricted(
            &ctx,
            TaskContextHandle::default(),
            program,
            Rc::new(Env::new()),
            policy(),
        )
        .expect("restricted evaluation uses ambient quantum");

        assert_eq!(value, Value::int(42));
        assert!(ctx.runtime_quantum_active());
        drop(outer_quantum);
        assert!(!ctx.runtime_quantum_active());
    }

    struct ReturnSecondCall {
        callable: Value,
    }

    impl Trace for ReturnSecondCall {
        fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            sink(GcEdge::Value(&self.callable));
            true
        }
    }

    impl NativeContinuation for ReturnSecondCall {
        fn resume(
            self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            input: ResumeInput,
        ) -> NativeResult {
            let ResumeInput::Returned(value) = input else {
                return Err(SemaError::eval("first restricted callback failed"));
            };
            Ok(NativeOutcome::Call(NativeCall {
                callable: self.callable,
                args: vec![value],
                continuation: Box::new(ReturnValue),
            }))
        }
    }

    struct ReturnValue;

    impl Trace for ReturnValue {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl NativeContinuation for ReturnValue {
        fn resume(
            self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            input: ResumeInput,
        ) -> NativeResult {
            match input {
                ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
                ResumeInput::Failed(error) => Err(error),
                _ => Err(SemaError::eval("restricted callback resumed unexpectedly")),
            }
        }
    }

    #[test]
    fn restricted_driver_drives_vm_callbacks_and_continuation_calls_inline() {
        let globals = Rc::new(Env::new());
        globals.set_str(
            "twice",
            Value::native_fn(
                NativeFn::simple_result("twice", |args| {
                    if args.len() != 2 {
                        return Err(SemaError::arity("twice", "2", args.len()));
                    }
                    Ok(NativeOutcome::Call(NativeCall {
                        callable: args[0].clone(),
                        args: vec![args[1].clone()],
                        continuation: Box::new(ReturnSecondCall {
                            callable: args[0].clone(),
                        }),
                    }))
                })
                .with_escaping_args(&[0]),
            ),
        );
        let forms =
            sema_reader::read_many("(twice (lambda (value) value) 41)").expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");
        let ctx = EvalContext::new();

        let value = run_program_restricted(
            &ctx,
            TaskContextHandle::default(),
            program,
            globals,
            policy(),
        )
        .expect("restricted callback chain succeeds");

        assert_eq!(value, Value::int(41));
        assert!(!ctx.runtime_quantum_active());
    }

    #[test]
    fn restricted_driver_drives_multimethod_dispatch_and_handler_inline() {
        let globals = Rc::new(Env::new());
        let dispatch = Value::native_fn(NativeFn::simple_result("dispatch", |_| {
            Ok(NativeOutcome::Return(Value::keyword("selected")))
        }));
        let selected = Value::native_fn(NativeFn::simple_result("selected", |args| {
            Ok(NativeOutcome::Return(args[0].clone()))
        }));
        let mut methods = BTreeMap::new();
        methods.insert(Value::keyword("selected"), selected);
        globals.set_str(
            "restricted-mm",
            Value::multimethod(MultiMethod {
                name: sema_core::intern("restricted-mm"),
                dispatch_fn: dispatch,
                methods: RefCell::new(methods),
                default: RefCell::new(None),
            }),
        );
        let forms = sema_reader::read_many("(restricted-mm 43)").expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");

        let value = run_program_restricted(
            &EvalContext::new(),
            TaskContextHandle::default(),
            program,
            globals,
            policy(),
        )
        .expect("restricted multimethod succeeds");

        assert_eq!(value, Value::int(43));
    }

    #[test]
    fn restricted_driver_drives_structural_keyword_calls_inline() {
        let globals = Rc::new(Env::new());
        globals.set_str(
            "lookup",
            Value::native_fn(NativeFn::simple_result("lookup", |_| {
                let mut map = BTreeMap::new();
                map.insert(Value::keyword("answer"), Value::int(44));
                Ok(NativeOutcome::Call(NativeCall {
                    callable: Value::keyword("answer"),
                    args: vec![Value::map(map)],
                    continuation: Box::new(ReturnValue),
                }))
            })),
        );
        let forms = sema_reader::read_many("(lookup)").expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");

        let value = run_program_restricted(
            &EvalContext::new(),
            TaskContextHandle::default(),
            program,
            globals,
            policy(),
        )
        .expect("restricted keyword call succeeds");

        assert_eq!(value, Value::int(44));
    }

    fn run_restricted_native(
        source: &str,
        name: &str,
        native: NativeFn,
    ) -> Result<Value, SemaError> {
        let globals = Rc::new(Env::new());
        globals.set_str(name, Value::native_fn(native));
        let forms = sema_reader::read_many(source).expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");
        run_program_restricted(
            &EvalContext::new(),
            TaskContextHandle::default(),
            program,
            globals,
            policy(),
        )
    }

    fn assert_cannot_suspend(source: &str, name: &str, native: NativeFn) {
        let error = run_restricted_native(source, name, native)
            .expect_err("restricted operation must reject suspension");
        assert!(matches!(
            error.inner(),
            SemaError::Eval(ref message) if message == "test evaluation cannot suspend"
        ));
    }

    #[test]
    fn restricted_driver_rejects_timer_through_runtime_abi_and_error_is_catchable() {
        let timer = || {
            NativeFn::simple_with_runtime(
                "timer",
                |_| Ok(Value::int(999)),
                |_, _| {
                    Ok(NativeOutcome::Suspend(NativeSuspend {
                        wait: WaitKind::Timer(Duration::from_secs(1)),
                        continuation: Box::new(ReturnValue),
                    }))
                },
            )
        };

        assert_cannot_suspend("(timer)", "timer", timer());
        let value = run_restricted_native("(try (timer) (catch error 45))", "timer", timer())
            .expect("parked VM catches restricted suspension error");
        assert_eq!(value, Value::int(45));
    }

    #[test]
    fn restricted_driver_rejects_channel_spawn_and_runtime_requests() {
        assert_cannot_suspend(
            "(make-channel)",
            "make-channel",
            NativeFn::simple_result("make-channel", |_| {
                Ok(NativeOutcome::Runtime(RuntimeRequest::CreateChannel {
                    capacity: 1,
                    continuation: Box::new(ReturnValue),
                }))
            }),
        );
        assert_cannot_suspend(
            "(spawn)",
            "spawn",
            NativeFn::simple_result("spawn", |_| {
                Ok(NativeOutcome::Runtime(RuntimeRequest::Spawn {
                    callable: Value::int(1),
                    continuation: Box::new(ReturnValue),
                }))
            }),
        );
        assert_cannot_suspend(
            "(barrier)",
            "barrier",
            NativeFn::simple_result("barrier", |_| {
                Ok(NativeOutcome::Runtime(RuntimeRequest::OriginBarrier {
                    continuation: Box::new(ReturnValue),
                }))
            }),
        );
    }

    struct RejectOnlyDecoder;

    impl Trace for RejectOnlyDecoder {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl CompletionDecoder for RejectOnlyDecoder {
        fn decode(
            self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            _result: Result<SendPayload, ExternalFailure>,
        ) -> Result<Value, SemaError> {
            panic!("rejected external operation must never be decoded")
        }
    }

    #[test]
    fn restricted_driver_rejects_external_operation_before_job_admission() {
        let job_runs = Arc::new(AtomicUsize::new(0));
        let native_runs = Arc::clone(&job_runs);
        assert_cannot_suspend(
            "(external)",
            "external",
            NativeFn::simple_result("external", move |_| {
                let job_runs = Arc::clone(&native_runs);
                Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::External(Box::new(
                        PreparedExternalOperation::quarantined_blocking(
                            CompletionKind::try_from_raw(97).expect("nonzero completion kind"),
                            Box::new(RejectOnlyDecoder),
                            QuarantineBound::hard_deadline(Duration::from_secs(1))
                                .expect("positive quarantine bound"),
                            move || {
                                job_runs.fetch_add(1, Ordering::SeqCst);
                                Ok(Box::new(()))
                            },
                        ),
                    )),
                    continuation: Box::new(ReturnValue),
                }))
            }),
        );
        assert_eq!(job_runs.load(Ordering::SeqCst), 0);
    }

    fn assert_eval_message(error: &SemaError, expected: &str) {
        assert!(matches!(
            error.inner(),
            SemaError::Eval(message) if message == expected
        ));
    }

    #[test]
    fn restricted_driver_enforces_one_shared_instruction_limit() {
        let globals = Rc::new(Env::new());
        let forms = sema_reader::read_many("(let loop () (loop))").expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");
        let mut limited = policy();
        limited.instruction_limit = NonZeroUsize::new(64).unwrap();

        let error = run_program_restricted(
            &EvalContext::new(),
            TaskContextHandle::default(),
            program,
            globals,
            limited,
        )
        .expect_err("unbounded bytecode must hit the instruction limit");

        assert_eval_message(&error, "test evaluation exceeded instruction limit");
    }

    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct ProbeContinuation {
        _probe: Rc<DropProbe>,
        resumes: Arc<AtomicUsize>,
    }

    impl Trace for ProbeContinuation {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl NativeContinuation for ProbeContinuation {
        fn resume(
            self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            input: ResumeInput,
        ) -> NativeResult {
            self.resumes.fetch_add(1, Ordering::SeqCst);
            match input {
                ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
                ResumeInput::Failed(error) => Err(error),
                _ => Err(SemaError::eval("probe resumed unexpectedly")),
            }
        }
    }

    #[test]
    fn restricted_driver_counts_continuation_resumes_and_drops_frames_at_the_cap() {
        let drops = Arc::new(AtomicUsize::new(0));
        let resumes = Arc::new(AtomicUsize::new(0));
        let globals = Rc::new(Env::new());
        globals.set_str(
            "step",
            Value::native_fn(NativeFn::simple_result("step", |_| {
                Ok(NativeOutcome::Return(Value::int(46)))
            })),
        );
        let start_drops = Arc::clone(&drops);
        let start_resumes = Arc::clone(&resumes);
        globals.set_str(
            "start",
            Value::native_fn(NativeFn::with_context_result("start", move |context, _| {
                let callable = context
                    .call_env
                    .as_ref()
                    .and_then(|env| env.get_str("step"))
                    .ok_or_else(|| SemaError::eval("step callable missing"))?;
                Ok(NativeOutcome::Call(NativeCall {
                    callable,
                    args: Vec::new(),
                    continuation: Box::new(ProbeContinuation {
                        _probe: Rc::new(DropProbe(Arc::clone(&start_drops))),
                        resumes: Arc::clone(&start_resumes),
                    }),
                }))
            })),
        );
        let forms = sema_reader::read_many("(start)").expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");
        let mut limited = policy();
        limited.transition_limit = NonZeroUsize::new(1).unwrap();
        let ctx = EvalContext::new();

        let error = run_program_restricted(
            &ctx,
            TaskContextHandle::default(),
            program,
            globals,
            limited,
        )
        .expect_err("continuation resume must consume the second transition");

        assert_eval_message(&error, "test evaluation exceeded transition limit");
        assert_eq!(resumes.load(Ordering::SeqCst), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(!ctx.runtime_quantum_active());
        assert!(ctx.task_context().is_none());
    }

    #[test]
    fn restricted_driver_checks_deadline_and_cancellation_before_a_quantum() {
        let run = |policy| {
            let forms = sema_reader::read_many("42").expect("source parses");
            let program = compile_program(&forms, None).expect("source compiles");
            let ctx = EvalContext::new();
            let result = run_program_restricted(
                &ctx,
                TaskContextHandle::default(),
                program,
                Rc::new(Env::new()),
                policy,
            );
            assert!(!ctx.runtime_quantum_active());
            assert!(ctx.task_context().is_none());
            result
        };

        let mut expired = policy();
        expired.deadline = Some(Instant::now());
        let error = run(expired).expect_err("expired deadline rejects before execution");
        assert_eval_message(&error, "test evaluation exceeded deadline");

        let mut cancelled = policy();
        cancelled.cancellation = CancellationView::new(true, Some(CancelReason::Explicit));
        let error = run(cancelled).expect_err("cancelled evaluation rejects before execution");
        assert_eval_message(&error, "test evaluation was cancelled");
    }

    struct RepeatCallback {
        callable: Value,
        remaining: usize,
    }

    impl Trace for RepeatCallback {
        fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            sink(GcEdge::Value(&self.callable));
            true
        }
    }

    impl NativeContinuation for RepeatCallback {
        fn resume(
            self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            input: ResumeInput,
        ) -> NativeResult {
            let ResumeInput::Returned(value) = input else {
                return Err(SemaError::eval("finite callback failed"));
            };
            if self.remaining == 0 {
                return Ok(NativeOutcome::Return(value));
            }
            Ok(NativeOutcome::Call(NativeCall {
                callable: self.callable.clone(),
                args: Vec::new(),
                continuation: Box::new(RepeatCallback {
                    callable: self.callable,
                    remaining: self.remaining - 1,
                }),
            }))
        }
    }

    fn run_finite_callbacks(count: usize, instruction_limit: usize) -> Result<Value, SemaError> {
        let globals = Rc::new(Env::new());
        globals.set_str(
            "run-count",
            Value::native_fn(
                NativeFn::simple_result("run-count", |args| {
                    let count = args
                        .get(1)
                        .and_then(Value::as_int)
                        .ok_or_else(|| SemaError::type_error("integer", "other"))?
                        as usize;
                    if count == 0 {
                        return Ok(NativeOutcome::Return(Value::nil()));
                    }
                    Ok(NativeOutcome::Call(NativeCall {
                        callable: args[0].clone(),
                        args: Vec::new(),
                        continuation: Box::new(RepeatCallback {
                            callable: args[0].clone(),
                            remaining: count - 1,
                        }),
                    }))
                })
                .with_escaping_args(&[0]),
            ),
        );
        let source = format!(
            "(run-count (lambda () (begin {})) {count})",
            (1..=48)
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
        let forms = sema_reader::read_many(&source).expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");
        let mut limited = policy();
        limited.instruction_limit = NonZeroUsize::new(instruction_limit).unwrap();
        run_program_restricted(
            &EvalContext::new(),
            TaskContextHandle::default(),
            program,
            globals,
            limited,
        )
    }

    #[test]
    fn restricted_driver_shares_one_instruction_budget_across_callback_vms() {
        let separating_limit = (1..=512)
            .find(|&limit| {
                run_finite_callbacks(1, limit).is_ok()
                    && run_finite_callbacks(2, limit).is_err_and(|error| {
                        matches!(
                            error.inner(),
                            SemaError::Eval(message)
                                if message == "test evaluation exceeded instruction limit"
                        )
                    })
            })
            .expect("finite callback fixture must expose a shared-budget boundary");

        assert_eq!(
            run_finite_callbacks(1, separating_limit).expect("one callback fits"),
            Value::int(48)
        );
        let error = run_finite_callbacks(2, separating_limit)
            .expect_err("two callbacks must share and exhaust the same instruction budget");
        assert_eval_message(&error, "test evaluation exceeded instruction limit");
    }

    struct DeepDropContinuation(Arc<AtomicUsize>);

    impl Drop for DeepDropContinuation {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Trace for DeepDropContinuation {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl NativeContinuation for DeepDropContinuation {
        fn resume(
            self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            input: ResumeInput,
        ) -> NativeResult {
            match input {
                ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
                ResumeInput::Failed(error) => Err(error),
                _ => Err(SemaError::eval("deep continuation resumed unexpectedly")),
            }
        }
    }

    #[test]
    fn restricted_driver_caps_and_cleans_up_a_deep_zero_bytecode_call_chain() {
        let calls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let native_calls = Arc::clone(&calls);
        let continuation_drops = Arc::clone(&drops);
        let globals = Rc::new(Env::new());
        globals.set_str(
            "call-forever",
            Value::native_fn(NativeFn::with_context_result(
                "call-forever",
                move |context, _| {
                    native_calls.fetch_add(1, Ordering::SeqCst);
                    let callable = context
                        .call_env
                        .as_ref()
                        .and_then(|env| env.get_str("call-forever"))
                        .ok_or_else(|| SemaError::eval("recursive callable missing"))?;
                    Ok(NativeOutcome::Call(NativeCall {
                        callable,
                        args: Vec::new(),
                        continuation: Box::new(DeepDropContinuation(Arc::clone(
                            &continuation_drops,
                        ))),
                    }))
                },
            )),
        );
        let forms = sema_reader::read_many("(call-forever)").expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");
        let mut limited = policy();
        let transition_limit = 50_000;
        limited.transition_limit = NonZeroUsize::new(transition_limit).unwrap();

        let error = run_program_restricted(
            &EvalContext::new(),
            TaskContextHandle::default(),
            program,
            globals,
            limited,
        )
        .expect_err("zero-bytecode call chain must hit the transition cap");

        assert_eval_message(&error, "test evaluation exceeded transition limit");
        assert_eq!(calls.load(Ordering::SeqCst), transition_limit + 1);
        assert_eq!(drops.load(Ordering::SeqCst), transition_limit + 1);
    }

    struct ContextMarker(usize);

    impl Trace for ContextMarker {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl TaskLocalValue for ContextMarker {
        fn inherit(&self) -> Rc<dyn TaskLocalValue> {
            Rc::new(Self(self.0))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct ContextContinuation(Arc<AtomicUsize>);

    impl Trace for ContextContinuation {
        fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            true
        }
    }

    impl NativeContinuation for ContextContinuation {
        fn resume(
            self: Box<Self>,
            context: &mut NativeCallContext<'_>,
            input: ResumeInput,
        ) -> NativeResult {
            assert_eq!(
                context
                    .task_context
                    .borrow()
                    .get::<ContextMarker>()
                    .map(|m| m.0),
                Some(77)
            );
            self.0.fetch_add(1, Ordering::SeqCst);
            match input {
                ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
                ResumeInput::Failed(error) => Err(error),
                _ => Err(SemaError::eval("context continuation resumed unexpectedly")),
            }
        }
    }

    #[test]
    fn restricted_driver_installs_exact_task_context_for_native_and_continuation() {
        let native_observations = Arc::new(AtomicUsize::new(0));
        let continuation_observations = Arc::new(AtomicUsize::new(0));
        let globals = Rc::new(Env::new());
        let step_observations = Arc::clone(&native_observations);
        globals.set_str(
            "context-step",
            Value::native_fn(NativeFn::with_context_result(
                "context-step",
                move |context, _| {
                    assert_eq!(
                        context
                            .task_context
                            .borrow()
                            .get::<ContextMarker>()
                            .map(|m| m.0),
                        Some(77)
                    );
                    step_observations.fetch_add(1, Ordering::SeqCst);
                    Ok(NativeOutcome::Return(Value::int(47)))
                },
            )),
        );
        let start_observations = Arc::clone(&native_observations);
        let continuation_seen = Arc::clone(&continuation_observations);
        globals.set_str(
            "context-start",
            Value::native_fn(NativeFn::with_context_result(
                "context-start",
                move |context, _| {
                    assert_eq!(
                        context
                            .task_context
                            .borrow()
                            .get::<ContextMarker>()
                            .map(|m| m.0),
                        Some(77)
                    );
                    start_observations.fetch_add(1, Ordering::SeqCst);
                    let callable = context
                        .call_env
                        .as_ref()
                        .and_then(|env| env.get_str("context-step"))
                        .ok_or_else(|| SemaError::eval("context step missing"))?;
                    Ok(NativeOutcome::Call(NativeCall {
                        callable,
                        args: Vec::new(),
                        continuation: Box::new(ContextContinuation(Arc::clone(&continuation_seen))),
                    }))
                },
            )),
        );
        let forms = sema_reader::read_many("(context-start)").expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");
        let inner = TaskContextHandle::default();
        inner.borrow_mut().insert(Rc::new(ContextMarker(77)));
        let outer = TaskContextHandle::default();
        outer.borrow_mut().insert(Rc::new(ContextMarker(11)));
        let ctx = EvalContext::new();
        ctx.install_task_context(outer);

        let value = run_program_restricted(&ctx, inner, program, globals, policy())
            .expect("context-aware restricted call succeeds");

        assert_eq!(value, Value::int(47));
        assert_eq!(native_observations.load(Ordering::SeqCst), 2);
        assert_eq!(continuation_observations.load(Ordering::SeqCst), 1);
        assert_eq!(
            ctx.task_context()
                .expect("outer task context restored")
                .borrow()
                .get::<ContextMarker>()
                .map(|m| m.0),
            Some(11)
        );
    }

    #[test]
    fn restricted_driver_rechecks_deadline_before_invoking_native_produced_call() {
        let starts = Arc::new(AtomicUsize::new(0));
        let steps = Arc::new(AtomicUsize::new(0));
        let globals = Rc::new(Env::new());
        let step_runs = Arc::clone(&steps);
        globals.set_str(
            "deadline-step",
            Value::native_fn(NativeFn::simple_result("deadline-step", move |_| {
                step_runs.fetch_add(1, Ordering::SeqCst);
                Ok(NativeOutcome::Return(Value::int(48)))
            })),
        );
        let start_runs = Arc::clone(&starts);
        globals.set_str(
            "deadline-start",
            Value::native_fn(NativeFn::with_context_result(
                "deadline-start",
                move |context, _| {
                    start_runs.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(150));
                    let callable = context
                        .call_env
                        .as_ref()
                        .and_then(|env| env.get_str("deadline-step"))
                        .ok_or_else(|| SemaError::eval("deadline step missing"))?;
                    Ok(NativeOutcome::Call(NativeCall {
                        callable,
                        args: Vec::new(),
                        continuation: Box::new(ReturnValue),
                    }))
                },
            )),
        );
        let forms = sema_reader::read_many("(deadline-start)").expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");
        let mut expiring = policy();
        expiring.deadline = Some(Instant::now() + Duration::from_millis(100));

        let error = run_program_restricted(
            &EvalContext::new(),
            TaskContextHandle::default(),
            program,
            globals,
            expiring,
        )
        .expect_err("deadline must expire before structural call invocation");

        assert_eval_message(&error, "test evaluation exceeded deadline");
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(steps.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn restricted_owner_storage_is_flat_for_nonrecursive_fatal_cleanup() {
        let source = include_str!("restricted.rs");
        let flat_owner_field = ["frames: Vec<", "RestrictedFrame>"].concat();
        let vm_index_field = ["vm_frame_indices: Vec<", "usize>"].concat();
        assert_eq!(source.matches(&flat_owner_field).count(), 1);
        assert_eq!(source.matches(&vm_index_field).count(), 1);
    }

    #[test]
    fn restricted_driver_rechecks_deadline_after_a_terminal_native_return() {
        let runs = Arc::new(AtomicUsize::new(0));
        let native_runs = Arc::clone(&runs);
        let globals = Rc::new(Env::new());
        globals.set_str(
            "deadline-return",
            Value::native_fn(NativeFn::simple_result("deadline-return", move |_| {
                native_runs.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(150));
                Ok(NativeOutcome::Return(Value::int(49)))
            })),
        );
        let forms = sema_reader::read_many("(deadline-return)").expect("source parses");
        let program = compile_program(&forms, None).expect("source compiles");
        let mut expiring = policy();
        expiring.deadline = Some(Instant::now() + Duration::from_millis(100));

        let error = run_program_restricted(
            &EvalContext::new(),
            TaskContextHandle::default(),
            program,
            globals,
            expiring,
        )
        .expect_err("deadline must be rechecked after the terminal native returns");

        assert_eval_message(&error, "test evaluation exceeded deadline");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }
}
