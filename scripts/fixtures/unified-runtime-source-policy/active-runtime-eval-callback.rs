fn active_runtime_eval_callback(ctx: &EvalContext) {
    if in_runtime_quantum() {
        eval_callback(ctx, &expr, &env);
    }
}
