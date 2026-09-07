//! Re-export the workflow checker from sema-stdlib so `main.rs` compiles
//! against a single implementation.
pub use sema_stdlib::workflow_check::{
    check_run_source, declared_permission_specs, report, Severity,
};
