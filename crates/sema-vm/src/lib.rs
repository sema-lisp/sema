#![allow(clippy::mutable_key_type)]
mod chunk;
mod compiler;
mod core_expr;
pub mod debug;
mod disasm;
mod emit;
mod lower;
mod opcodes;
mod optimize;
mod resolve;
mod scheduler;
mod serialize;
mod vm;

pub use chunk::{Chunk, ExceptionEntry, Function, UpvalueDesc};
pub use compiler::{compile, CompileResult};
pub use core_expr::{
    CoreExpr, DoLoop, DoVar, Expr, LambdaDef, PromptEntry, ResolvedExpr, VarRef, VarResolution,
};
pub use debug::{
    decode_scope_ref, scope_locals_ref, scope_upvalues_ref, DapBreakpoint, DebugCommand,
    DebugEvent, DebugState, ScopeKind, SourceBreakpoint, StepMode, StopInfo, StopReason,
    VmExecResult,
};
pub use disasm::disassemble;
pub use emit::Emitter;
pub use lower::lower;
pub use opcodes::Op;
pub use optimize::optimize as optimize_expr;
pub use resolve::resolve_with_locals;
pub use scheduler::{
    init_scheduler, reset_scheduler_tasks, scheduler_task_count, shutdown_scheduler,
    with_coop_paused_task_vm,
};
pub use serialize::{deserialize_from_bytes, is_bytecode_file, serialize_to_bytes};
pub use vm::{
    compile_program, compile_program_with_spans, compile_program_with_spans_and_natives,
    current_vm_globals, extract_vm_closure, is_debug_session_active, snap_breakpoint_line,
    valid_breakpoint_lines, valid_breakpoint_lines_by_file, with_active_debug, Closure,
    CompiledProgram, DebugStopResume, UpvalueCell, UpvalueState, VM,
};
