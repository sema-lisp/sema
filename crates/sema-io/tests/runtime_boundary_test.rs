//! The generic blocking adapter is legal only when no VM runtime quantum is
//! active on the calling thread.

use sema_core::EvalContext;

#[test]
fn io_block_on_rejects_an_active_runtime_quantum() {
    let ctx = EvalContext::new();
    let _quantum = ctx.enter_runtime_quantum().expect("enter runtime quantum");

    let rejected = std::panic::catch_unwind(|| sema_io::io_block_on(async { 42 }));

    assert!(rejected.is_err(), "io_block_on must be a host-only adapter");
}

#[test]
fn io_block_on_remains_available_to_a_plain_host_thread() {
    assert_eq!(sema_io::io_block_on(async { 42 }), 42);
}
