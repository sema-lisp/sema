use sema_core::{check_arity, EvalContext, NativeFn, SemaError, Value};

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
    register_fn_ctx(env, "context/with", |ctx, args| {
        check_arity!(args, "context/with", 2);
        let bindings = args[0]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
        let thunk = &args[1];
        if thunk.as_lambda_rc().is_none() && thunk.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", thunk.type_name()));
        }

        let frame: std::collections::BTreeMap<Value, Value> = bindings
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        ctx.context_push_frame_with(frame);
        let result = sema_core::call_callback(ctx, thunk, &[]);
        ctx.context_pop_frame();
        result
    });
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
