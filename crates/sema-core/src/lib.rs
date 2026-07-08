#![allow(clippy::mutable_key_type)]
pub mod archive;
pub mod async_signal;
pub mod context;
pub mod cycle;
pub mod error;
pub mod home;
pub mod io_backend;
pub mod json;
pub mod mcp_cassette;
pub mod net;
pub mod num;
pub mod number;
pub mod output_hook;
pub mod resolve;
pub mod sandbox;
pub mod stack;
pub mod text_util;
pub mod value;
pub mod vfs;

pub use async_signal::{
    blocking_sleep_ms, call_cancel_callback, call_run_scheduler, call_run_scheduler_all_of,
    call_run_scheduler_any_of, call_run_scheduler_target, call_run_scheduler_timeout,
    call_spawn_callback, check_interrupt, clear_blocking_sleep_callback, clear_interrupt_callback,
    current_conversation_scope_boxed, current_llm_scope_boxed, current_task_id,
    current_usage_scope_boxed, debug_coop_resume_pending, in_async_context, install_task_llm_scope,
    install_task_otel, install_task_usage_scope, io_park, notify_io_complete, notify_task_reaped,
    set_async_context, set_blocking_sleep_callback, set_cancel_callback, set_current_task_id,
    set_debug_coop_resume, set_interrupt_callback, set_llm_scope_task_callbacks,
    set_otel_task_callbacks, set_resume_value, set_run_scheduler_callback, set_spawn_callback,
    set_task_reaped_callback, set_usage_scope_task_callbacks, set_yield_signal,
    take_debug_coop_resume, take_resume_value, take_task_llm_scope, take_task_otel,
    take_task_usage_scope, take_yield_signal, BlockingSleepFn, CancelCallbackFn, DebugCoopResume,
    InterruptCallbackFn, IoHandle, IoPoll, LlmScopeCaptureFn, LlmScopeInstallFn, LlmScopeTakeFn,
    OtelInstallFn, OtelScopeFn, OtelTakeFn, RunSchedulerCallbackFn, SchedulerRunResult,
    SchedulerTarget, SpawnCallbackFn, TaskReapedFn, UsageScopeCaptureFn, UsageScopeInstallFn,
    UsageScopeTakeFn, YieldReason,
};
pub use context::{
    call_callback, eval_callback, set_call_callback, set_eval_callback, with_stdlib_ctx,
    CallCallbackFn, EvalCallbackFn, EvalContext,
};
pub use cycle::{
    collect as gc_collect, env_chain_pins as gc_env_chain_pins, last_stats as gc_last_stats,
    maybe_collect as gc_maybe_collect, register_candidate, register_closure_birth,
    register_env_candidate, register_payload_tracer, registry_len as gc_registry_len,
    set_gc_observer, should_collect as gc_should_collect,
    threshold_collect as gc_threshold_collect, trace_value, EnvBindings, GcEdge, GcNode,
    GcPassEvent, GcStats, GcTrigger, NodePtr, OpaqueSeverFn, OpaqueTraceFn, PayloadTracer,
};
pub use error::{CallFrame, SemaError, Span, SpanMap, StackTrace};
pub use home::sema_home;
pub use io_backend::{
    io_backend, io_block_on, io_spawn, io_spawn_blocking, set_io_backend, AbortHook, BoxIoFuture,
    IoBackend,
};
pub use json::{json_to_value, key_to_string, value_to_json, value_to_json_lossy};
pub use lasso::Spur;
pub use mcp_cassette::{
    clear_mcp_cassette_hook, mcp_cassette_decide, mcp_cassette_record, set_mcp_cassette_hook,
    McpCassetteDecision,
};
pub use output_hook::{set_stderr_hook, set_stdout_hook, write_stderr, write_stdout};
pub use sandbox::{Caps, Sandbox};
pub use text_util::truncate_chars;
pub use value::{
    bits_to_spur, compare_spurs, intern, interner_stats, next_gensym, pretty_print, resolve,
    spur_to_bits, with_resolved, Agent, AsyncPromise, Channel, Conversation, Env, ImageAttachment,
    Lambda, Macro, Message, MultiMethod, NativeFn, PromiseState, Prompt, Record, Role, SemaStream,
    StreamBox, Thunk, ToolDefinition, Value, ValueView, ValueViewRef, NAN_INT_SIGN_BIT,
    NAN_INT_SMALL_PATTERN, NAN_PAYLOAD_BITS, NAN_PAYLOAD_MASK, NAN_TAG_MASK, TAG_NATIVE_FN,
};
