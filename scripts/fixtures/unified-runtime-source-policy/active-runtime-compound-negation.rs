fn active_runtime_compound_negation(ctx: &EvalContext, force: bool) {
    if !in_runtime_quantum() || force {
        call_callback(ctx, &func, &args);
    }
}
