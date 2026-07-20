fn active_runtime_multiline_callback(ctx: &EvalContext) {
    if in_runtime_quantum() {
        call_callback(ctx, &func, &args);
    }
}
