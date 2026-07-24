fn negated_runtime_host_adapter(ctx: &EvalContext) {
    if !in_runtime_quantum() {
        call_callback(ctx, &func, &args);
    }
}
