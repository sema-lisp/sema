fn unallowlisted_host_adapter(ctx: &EvalContext) {
    let callback: EvalCallbackFn = eval;
    set_eval_callback(ctx, callback);
    eval_callback(ctx, &expr, &env);
    ctx.eval_fn.set(Some(callback));
    call_callback(ctx, &func, &args);
    call_callback_owned(ctx, &func, &mut args);
    with_stdlib_ctx(|ctx| callback(ctx));
    set_call_callback(ctx, call);
    set_call_owned_callback(ctx, call_owned);
}
