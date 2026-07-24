fn active_runtime_workflow_tls() {
    if in_runtime_quantum() {
        WORKFLOW.with(|state| state.current_ctx());
    }
}
