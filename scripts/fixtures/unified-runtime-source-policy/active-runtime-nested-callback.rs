fn active_runtime_nested_callback(ctx: &EvalContext, ready: bool) {
    if in_runtime_quantum() {
        if ready {
            prepare();
        }
        call_callback(ctx, &func, &args);
    }
}
