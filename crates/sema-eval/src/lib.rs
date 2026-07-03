#![allow(clippy::mutable_key_type)]
mod debug_session;
mod destructure;
mod eval;
mod prelude;
mod special_forms;

pub use debug_session::{is_debug_session_active, set_debug_session_active};
pub use eval::{
    call_value, create_module_env, eval_module_body_vm, eval_value_vm, execute_compile_result,
    load_prelude, register_vm_delegates, EvalResult, Interpreter, Trampoline,
};
pub use sema_core::EvalContext;
pub use special_forms::SPECIAL_FORM_NAMES;
