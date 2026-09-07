use super::*;

pub(super) fn register_fn(
    env: &Env,
    name: &str,
    f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::simple(name, f)),
    );
}

pub(super) fn register_fn_ctx(
    env: &Env,
    name: &str,
    f: impl Fn(&EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::with_ctx(name, f)),
    );
}

/// Register a dual-ABI native whose body speaks the runtime native ABI
/// (`NativeResult`) so its `in_runtime_quantum` branch can return a
/// `NativeOutcome::Suspend`/`Call` directly (an external-wait offload or a
/// cooperative tool round). The runtime callback uses the exact evaluator
/// context carried by its [`sema_core::runtime::NativeCallContext`]. The plain
/// value callback runs the same body with its evaluator context and unwraps the
/// `Return` produced outside a runtime quantum.
///
/// True when a provider call must offload so the interpreter thread never
/// blocks. Root-main and addressable spawned tasks are both suspendable runtime
/// work; `current_task_id()` distinguishes cancellation handles, not whether a
/// quantum may park.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn in_runtime_offload_context() -> bool {
    sema_core::in_runtime_quantum()
}

pub(super) fn register_runtime_fn_ctx(
    env: &Env,
    name: &str,
    f: impl Fn(&EvalContext, &[Value]) -> sema_core::runtime::NativeResult + 'static,
) {
    use sema_core::runtime::NativeOutcome;
    let f = std::rc::Rc::new(f);
    let for_func = f.clone();
    let for_runtime = f;
    let err_name = name.to_string();
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::with_ctx_runtime(
            name,
            move |ctx, args| match for_func(ctx, args)? {
                NativeOutcome::Return(value) => Ok(value),
                _ => Err(SemaError::eval(format!(
                    "{err_name}: native suspended outside the cooperative runtime"
                ))),
            },
            move |native_ctx, args| for_runtime(native_ctx.eval_context, args),
        )),
    );
}

/// Like [`register_runtime_fn_ctx`], but capability-gated and value-args (no
/// `EvalContext` in the body). The gate runs on BOTH ABI callbacks so a sandboxed
/// caller sees the same `PermissionDenied` whether the native is reached through
/// its plain value callback or its runtime callback.
pub(super) fn register_runtime_fn_gated(
    env: &Env,
    sandbox: &sema_core::Sandbox,
    cap: sema_core::Caps,
    name: &str,
    f: impl Fn(&[Value]) -> sema_core::runtime::NativeResult + 'static,
) {
    use sema_core::runtime::NativeOutcome;
    type RuntimeFnBody = dyn Fn(&[Value]) -> sema_core::runtime::NativeResult;
    let body: std::rc::Rc<RuntimeFnBody> = if sandbox.is_unrestricted() {
        std::rc::Rc::new(f)
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        std::rc::Rc::new(move |args: &[Value]| {
            sandbox.check(cap, &fn_name)?;
            f(args)
        })
    };
    let for_func = body.clone();
    let for_runtime = body;
    let err_name = name.to_string();
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::simple_with_runtime(
            name,
            move |args| match for_func(args)? {
                NativeOutcome::Return(value) => Ok(value),
                _ => Err(SemaError::eval(format!(
                    "{err_name}: native suspended outside the cooperative runtime"
                ))),
            },
            move |_native_ctx, args| for_runtime(args),
        )),
    );
}

/// Cooperative teardown for an `llm/with-*` dynamic-scope wrapper. The wrapper's
/// setup installs the scope's thread-local state and hands this continuation a
/// `teardown` closure that restores the prior state; the runtime drives the
/// wrapped thunk as a `NativeOutcome::Call` (so an async op inside it parks on the
/// active task instead of hitting the runtime-only error stub) and resumes this
/// continuation with the thunk's result. Teardown runs on return, failure, AND
/// cancellation, then the original outcome is propagated so an enclosing
/// try/catch sees the same value/error as the synchronous path. The teardown
/// closure captures only non-`Value` scope state (bools/ints/`Rc<BudgetFrame>`/
/// provider names), so there are no GC edges to trace.
pub(super) struct ScopeGuardContinuation {
    pub(super) teardown: Option<Box<dyn FnOnce()>>,
}

impl sema_core::runtime::Trace for ScopeGuardContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        true
    }
}

impl sema_core::runtime::NativeContinuation for ScopeGuardContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut sema_core::runtime::NativeCallContext<'_>,
        input: sema_core::runtime::ResumeInput,
    ) -> sema_core::runtime::NativeResult {
        use sema_core::runtime::{NativeOutcome, ResumeInput};
        if let Some(teardown) = self.teardown.take() {
            teardown();
        }
        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "llm/with-* thunk was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "llm/with-* teardown received an unexpected runtime response",
            )),
        }
    }
}

/// Register an `llm/with-*` dynamic-scope wrapper as a dual-ABI native. `setup`
/// validates the args, installs the scope's thread-local state, and returns the
/// body thunk plus a teardown closure that restores the prior state.
///
/// Under the unified runtime the body runs as a cooperative
/// `NativeOutcome::Call`, so an async op inside the thunk (`async/spawn`,
/// `channel/*`, …) parks on the active task and works — where a synchronous
/// `call_callback` re-entry would suspend the runtime quantum and hit the
/// runtime-only error stub. Teardown runs when the thunk RETURNS (matching the
/// synchronous `call_callback` extent): a thunk that only builds a promise tears down
/// immediately; a thunk that itself awaits keeps the scope installed across the
/// await (the thread-local is not per-task-swapped mid-quantum). Outside a runtime
/// quantum the plain value callback runs the thunk synchronously and tears down
/// inline.
pub(super) fn register_scope_fn_ctx(
    env: &Env,
    name: &'static str,
    setup: impl Fn(&[Value]) -> Result<(Value, Box<dyn FnOnce()>), SemaError> + 'static,
) {
    use sema_core::runtime::{NativeCall, NativeOutcome};
    let setup = std::rc::Rc::new(setup);
    let for_func = setup.clone();
    let for_runtime = setup;
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::with_ctx_runtime(
            name,
            move |ctx, args| {
                let (body_fn, teardown) = for_func(args)?;
                let result = sema_core::call_callback(ctx, &body_fn, &[]);
                teardown();
                result
            },
            move |_native_ctx, args| {
                let (body_fn, teardown) = for_runtime(args)?;
                Ok(NativeOutcome::Call(NativeCall {
                    callable: body_fn,
                    args: Vec::new(),
                    continuation: Box::new(ScopeGuardContinuation {
                        teardown: Some(teardown),
                    }),
                }))
            },
        )),
    );
}

/// Bind a capability-gated context native under a different ENV SYMBOL
/// (`reg_name`) than the name used for the capability-check error / `NativeFn`
/// display (`display_name`). Used to split a Sema-visible entry point (e.g.
/// `llm/chat`) into several internal native entry points (a blocking twin, an
/// async-loop `-begin`) that must all deny with the SAME `PermissionDenied {
/// function: "llm/chat", .. }` a sandboxed caller saw before the split — the
/// prelude dispatcher decides at runtime which internal native actually runs, but
/// every one of them gates identically under the public name.
pub(super) fn register_fn_ctx_gated_as(
    env: &Env,
    sandbox: &sema_core::Sandbox,
    cap: sema_core::Caps,
    reg_name: &str,
    display_name: &str,
    f: impl Fn(&sema_core::EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
) {
    if sandbox.is_unrestricted() {
        env.set(
            sema_core::intern(reg_name),
            Value::native_fn(NativeFn::with_ctx(display_name, f)),
        );
    } else {
        let sandbox = sandbox.clone();
        let fn_name = display_name.to_string();
        env.set(
            sema_core::intern(reg_name),
            Value::native_fn(NativeFn::with_ctx(display_name, move |ctx, args| {
                sandbox.check(cap, &fn_name)?;
                f(ctx, args)
            })),
        );
    }
}

/// Runtime-ABI sibling of [`register_fn_ctx_gated_as`]: registers under
/// `reg_name` a dual-ABI native whose body speaks `NativeResult` (so its
/// `in_runtime_quantum` branch can `NativeOutcome::Suspend`), gated as
/// `display_name`. The runtime callback receives the invoking runtime's exact
/// evaluator context (as [`register_runtime_fn_ctx`] does); the plain value
/// callback runs the same body with its evaluator context and unwraps the
/// `Return`.
pub(super) fn register_runtime_fn_ctx_gated_as(
    env: &Env,
    sandbox: &sema_core::Sandbox,
    cap: sema_core::Caps,
    reg_name: &str,
    display_name: &str,
    f: impl Fn(&EvalContext, &[Value]) -> sema_core::runtime::NativeResult + 'static,
) {
    use sema_core::runtime::NativeOutcome;
    let f = std::rc::Rc::new(f);
    let for_func = f.clone();
    let for_runtime = f;
    let err_name = display_name.to_string();
    let unrestricted = sandbox.is_unrestricted();
    let sandbox_func = sandbox.clone();
    let sandbox_runtime = sandbox.clone();
    let gate_name_func = display_name.to_string();
    let gate_name_runtime = display_name.to_string();
    env.set(
        sema_core::intern(reg_name),
        Value::native_fn(NativeFn::with_ctx_runtime(
            display_name,
            move |ctx, args| {
                if !unrestricted {
                    sandbox_func.check(cap, &gate_name_func)?;
                }
                match for_func(ctx, args)? {
                    NativeOutcome::Return(value) => Ok(value),
                    _ => Err(SemaError::eval(format!(
                        "{err_name}: native suspended outside the cooperative runtime"
                    ))),
                }
            },
            move |native_ctx, args| {
                if !unrestricted {
                    sandbox_runtime.check(cap, &gate_name_runtime)?;
                }
                for_runtime(native_ctx.eval_context, args)
            },
        )),
    );
}
