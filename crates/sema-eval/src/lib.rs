#![allow(clippy::mutable_key_type)]
mod debug_session;
mod destructure;
mod eval;
#[cfg(not(target_arch = "wasm32"))]
mod host;
mod prelude;
mod special_forms;
mod syntax_rules;

pub use debug_session::{is_debug_session_active, set_debug_session_active};
pub use eval::{
    call_value, call_value_owned, create_module_env, eval_module_body_vm, eval_value_vm,
    execute_compile_result, load_prelude, register_vm_delegates, EvalResult, Interpreter,
    Trampoline,
};
#[cfg(not(target_arch = "wasm32"))]
pub use host::{spawn_host_thread, CancelToken};
pub use sema_core::EvalContext;
pub use special_forms::SPECIAL_FORM_NAMES;
