fn permitted_host_adapter(ctx: &EvalContext) {
    call_callback(ctx, &func, &args);
    call_callback_owned(ctx, &func, &mut args);
    with_stdlib_ctx(|ctx| callback(ctx));
    set_call_callback(ctx, call);
    set_call_owned_callback(ctx, call_owned);
}
