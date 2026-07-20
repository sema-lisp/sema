fn active_context_eval_callback(ctx: &EvalContext) {
    if ctx.runtime_quantum_active() {
        eval_callback(ctx, &expr, &env);
    }
}
