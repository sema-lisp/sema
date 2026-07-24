fn active_context_callback(ctx: &EvalContext) {
    if ctx.runtime_quantum_active() {
        call_callback(ctx, &func, &args);
    }
}
