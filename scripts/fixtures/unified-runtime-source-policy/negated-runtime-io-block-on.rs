fn negated_runtime_io_block_on(fut: SomeFuture) {
    if !in_runtime_quantum() {
        let _ = io_block_on(fut);
    }
}
