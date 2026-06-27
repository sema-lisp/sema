//! Re-export the workflow checker from sema-stdlib so `main.rs` compiles
//! against a single implementation.
pub use sema_stdlib::workflow_check::{check_source, report};
// Diag and Severity are exported for callers that inspect diagnostics directly.
#[allow(unused_imports)]
pub use sema_stdlib::workflow_check::{Diag, Severity};
