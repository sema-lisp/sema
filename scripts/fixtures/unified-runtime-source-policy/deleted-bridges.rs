fn deleted_bridge_surface() {
    let _ = CURRENT_VM;
    let _ = CurrentVmGuard;
    try_run_on_current_vm();
    try_run_on_current_vm_args();
    run_nested_closure_args();
    current_vm_globals();
    suspend_runtime_quantum();
    let _ = QuantumSuspendGuard;
    snapshot_escaping_closure();
    snapshot_escaping_value();
    snapshot_native_escaping_args_for_current_vm();
}
