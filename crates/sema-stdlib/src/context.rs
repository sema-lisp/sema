use std::collections::BTreeMap;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    DynamicTaskState, NativeCall, NativeCallContext, NativeContinuation, NativeOutcome,
    NativeResult, ResumeInput, ScopeId, Trace,
};
use sema_core::{check_arity, EvalContext, NativeFn, SemaError, Value};

fn context_with_args(args: &[Value]) -> Result<(BTreeMap<Value, Value>, Value), SemaError> {
    check_arity!(args, "context/with", 2);
    let bindings = args[0]
        .as_map_rc()
        .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
    let thunk = &args[1];
    if thunk.as_lambda_rc().is_none() && thunk.as_native_fn_rc().is_none() {
        return Err(SemaError::type_error("function", thunk.type_name()));
    }

    Ok((
        bindings
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        thunk.clone(),
    ))
}

fn context_with_legacy(ctx: &EvalContext, args: &[Value]) -> Result<Value, SemaError> {
    let (frame, thunk) = context_with_args(args)?;
    ctx.context_push_frame_with(frame);
    let result = sema_core::call_callback(ctx, &thunk, &[]);
    ctx.context_pop_frame();
    result
}

struct ContextWithContinuation {
    scope: ScopeId,
}

impl Trace for ContextWithContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for ContextWithContinuation {
    fn resume(
        self: Box<Self>,
        context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let state = context
            .task_context
            .get::<DynamicTaskState>()
            .ok_or_else(|| SemaError::eval("context/with resumed without dynamic task state"))?;
        // `context/clear` deliberately removes every frame, including this
        // owned one. Teardown is exact and idempotent: remove this identity if
        // it still exists, while accepting that the thunk already removed it.
        state.remove_user_frame(self.scope);

        match input {
            ResumeInput::Returned(value) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                "context/with thunk was cancelled ({reason:?})"
            ))),
            ResumeInput::Runtime(_) => Err(SemaError::eval(
                "context/with teardown received an unexpected runtime response",
            )),
        }
    }
}

fn context_with_runtime(context: &mut NativeCallContext<'_>, args: &[Value]) -> NativeResult {
    let (frame, thunk) = context_with_args(args)?;
    let state = context
        .task_context
        .get::<DynamicTaskState>()
        .ok_or_else(|| SemaError::eval("context/with requires dynamic task state"))?;
    let scope = state
        .push_user_frame(frame)
        .map_err(|error| SemaError::eval(format!("context/with: {error}")))?;
    Ok(NativeOutcome::Call(NativeCall {
        callable: thunk,
        args: Vec::new(),
        continuation: Box::new(ContextWithContinuation { scope }),
    }))
}

pub fn register(env: &sema_core::Env) {
    register_fn_ctx_with_escaping_args(env, "context/set", &[0, 1], |ctx, args| {
        check_arity!(args, "context/set", 2);
        ctx.context_set(args[0].clone(), args[1].clone());
        Ok(Value::nil())
    });

    register_fn_ctx(env, "context/get", |ctx, args| {
        check_arity!(args, "context/get", 1);
        Ok(ctx.context_get(&args[0]).unwrap_or_else(Value::nil))
    });

    register_fn_ctx(env, "context/has?", |ctx, args| {
        check_arity!(args, "context/has?", 1);
        Ok(Value::bool(ctx.context_has(&args[0])))
    });

    register_fn_ctx(env, "context/remove", |ctx, args| {
        check_arity!(args, "context/remove", 1);
        Ok(ctx.context_remove(&args[0]).unwrap_or_else(Value::nil))
    });

    register_fn_ctx(env, "context/all", |ctx, args| {
        check_arity!(args, "context/all", 0);
        Ok(Value::map(ctx.context_all()))
    });

    register_fn_ctx(env, "context/pull", |ctx, args| {
        check_arity!(args, "context/pull", 1);
        Ok(ctx.context_remove(&args[0]).unwrap_or_else(Value::nil))
    });

    register_fn_ctx_with_escaping_args(env, "context/push", &[0, 1], |ctx, args| {
        check_arity!(args, "context/push", 2);
        ctx.context_stack_push(args[0].clone(), args[1].clone());
        Ok(Value::nil())
    });

    register_fn_ctx(env, "context/stack", |ctx, args| {
        check_arity!(args, "context/stack", 1);
        Ok(Value::list(ctx.context_stack_get(&args[0])))
    });

    register_fn_ctx(env, "context/pop", |ctx, args| {
        check_arity!(args, "context/pop", 1);
        Ok(ctx.context_stack_pop(&args[0]).unwrap_or_else(Value::nil))
    });

    register_fn_ctx_with_escaping_args(env, "context/set-hidden", &[0, 1], |ctx, args| {
        check_arity!(args, "context/set-hidden", 2);
        ctx.hidden_set(args[0].clone(), args[1].clone());
        Ok(Value::nil())
    });

    register_fn_ctx(env, "context/get-hidden", |ctx, args| {
        check_arity!(args, "context/get-hidden", 1);
        Ok(ctx.hidden_get(&args[0]).unwrap_or_else(Value::nil))
    });

    register_fn_ctx(env, "context/has-hidden?", |ctx, args| {
        check_arity!(args, "context/has-hidden?", 1);
        Ok(Value::bool(ctx.hidden_has(&args[0])))
    });

    register_fn_ctx_with_escaping_args(env, "context/merge", &[0], |ctx, args| {
        check_arity!(args, "context/merge", 1);
        let map = args[0]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
        for (k, v) in map.iter() {
            ctx.context_set(k.clone(), v.clone());
        }
        Ok(Value::nil())
    });

    register_fn_ctx(env, "context/clear", |ctx, args| {
        check_arity!(args, "context/clear", 0);
        ctx.context_clear();
        Ok(Value::nil())
    });

    // (context/with bindings-map thunk) -> result of thunk
    env.set(
        sema_core::intern("context/with"),
        Value::native_fn(
            NativeFn::with_ctx_runtime("context/with", context_with_legacy, context_with_runtime)
                .with_escaping_args(&[0, 1]),
        ),
    );
}

fn register_fn_ctx(
    env: &sema_core::Env,
    name: &str,
    f: impl Fn(&EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::with_ctx(name, f)),
    );
}

fn register_fn_ctx_with_escaping_args(
    env: &sema_core::Env,
    name: &str,
    escaping_args: &'static [usize],
    f: impl Fn(&EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::with_ctx(name, f).with_escaping_args(escaping_args)),
    );
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use sema_core::runtime::{CancellationView, RuntimeResponse, TaskContext};

    use super::*;

    #[test]
    fn context_with_continuation_cleans_exact_scope_on_unexpected_response() {
        let key = Value::keyword("scoped");
        let state_a = Rc::new(DynamicTaskState::root(
            vec![BTreeMap::from([(key.clone(), Value::keyword("outer-a"))])],
            vec![BTreeMap::new()],
            BTreeMap::new(),
        ));
        let state_b = Rc::new(DynamicTaskState::root(
            vec![BTreeMap::from([(key.clone(), Value::keyword("outer-b"))])],
            vec![BTreeMap::new()],
            BTreeMap::new(),
        ));
        let scope_a = state_a
            .push_user_frame(BTreeMap::from([(key.clone(), Value::keyword("inner-a"))]))
            .expect("scope A");
        let scope_b = state_b
            .push_user_frame(BTreeMap::from([(key.clone(), Value::keyword("inner-b"))]))
            .expect("scope B");
        assert_eq!(scope_a, scope_b, "fresh task-local counters collide");

        let eval_a = EvalContext::new();
        let mut task_a = TaskContext::empty();
        task_a.insert(Rc::clone(&state_a));
        let mut call_a = NativeCallContext {
            eval_context: &eval_a,
            task_context: &mut task_a,
            cancellation: CancellationView::default(),
        };
        let result = Box::new(ContextWithContinuation { scope: scope_a }).resume(
            &mut call_a,
            ResumeInput::Runtime(RuntimeResponse::Value(Value::nil())),
        );

        assert!(result.is_err());
        assert_eq!(state_a.user_get(&key), Some(Value::keyword("outer-a")));
        assert_eq!(state_b.user_get(&key), Some(Value::keyword("inner-b")));

        let eval_b = EvalContext::new();
        let mut task_b = TaskContext::empty();
        task_b.insert(Rc::clone(&state_b));
        let mut call_b = NativeCallContext {
            eval_context: &eval_b,
            task_context: &mut task_b,
            cancellation: CancellationView::default(),
        };
        let result = Box::new(ContextWithContinuation { scope: scope_b })
            .resume(&mut call_b, ResumeInput::Returned(Value::int(7)))
            .expect("scope B returns");
        assert!(matches!(result, NativeOutcome::Return(value) if value == Value::int(7)));
        assert_eq!(state_b.user_get(&key), Some(Value::keyword("outer-b")));
    }

    #[test]
    fn context_with_runtime_traces_the_parked_frame_and_callable() {
        let state = Rc::new(DynamicTaskState::root(
            vec![BTreeMap::new()],
            vec![BTreeMap::new()],
            BTreeMap::new(),
        ));
        let eval = EvalContext::new();
        let mut task = TaskContext::empty();
        task.insert(Rc::clone(&state));
        let mut call_context = NativeCallContext {
            eval_context: &eval,
            task_context: &mut task,
            cancellation: CancellationView::default(),
        };
        let outcome = context_with_runtime(
            &mut call_context,
            &[
                Value::map(BTreeMap::from([(
                    Value::keyword("key"),
                    Value::string("value"),
                )])),
                Value::native_fn(NativeFn::simple("thunk", |_| Ok(Value::nil()))),
            ],
        )
        .expect("context/with runtime call");
        let NativeOutcome::Call(call) = outcome else {
            panic!("context/with must return a structural call");
        };

        let mut task_edges = 0;
        assert!(task.trace(&mut |_| task_edges += 1));
        assert_eq!(task_edges, 2, "task state traces the scoped key and value");

        let mut call_edges = 0;
        assert!(call.trace(&mut |_| call_edges += 1));
        assert_eq!(call_edges, 1, "native call traces the parked thunk");

        let mut continuation_edges = 0;
        assert!(call.continuation.trace(&mut |_| continuation_edges += 1));
        assert_eq!(continuation_edges, 0, "scope token carries no Value edge");
    }
}
