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
pub mod mutable_ops;
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
    blocking_sleep_ms, check_interrupt, clear_blocking_sleep_callback, clear_interrupt_callback,
    current_conversation_scope_boxed, current_llm_scope_boxed, current_task_id,
    current_usage_scope_boxed, in_runtime_quantum, install_task_llm_scope, install_task_otel,
    install_task_usage_scope, llm_scope_ambient_is_empty, llm_scope_captured_is_empty,
    notify_task_reaped, otel_ambient_is_empty, otel_captured_is_empty, set_blocking_sleep_callback,
    set_current_task_id, set_interrupt_callback, set_llm_scope_empty_callbacks,
    set_llm_scope_task_callbacks, set_otel_empty_callbacks, set_otel_task_callbacks,
    set_runtime_quantum, set_task_reaped_callback, set_usage_scope_empty_callbacks,
    set_usage_scope_task_callbacks, take_task_llm_scope, take_task_otel, take_task_usage_scope,
    usage_scope_ambient_is_empty, usage_scope_captured_is_empty, BlockingSleepFn,
    InterruptCallbackFn, LlmScopeAmbientEmptyFn, LlmScopeCaptureFn, LlmScopeInstallFn,
    LlmScopeIsEmptyFn, LlmScopeTakeFn, OtelAmbientEmptyFn, OtelInstallFn, OtelIsEmptyFn,
    OtelScopeFn, OtelTakeFn, TaskReapedFn, UsageScopeAmbientEmptyFn, UsageScopeCaptureFn,
    UsageScopeInstallFn, UsageScopeIsEmptyFn, UsageScopeTakeFn,
};
pub use context::{
    call_callback, call_callback_owned, eval_callback, set_call_callback, set_call_owned_callback,
    set_eval_callback, set_macro_expand_callback, try_macro_expand_callback, with_stdlib_ctx,
    CallCallbackFn, CallOwnedCallbackFn, EvalCallbackFn, EvalContext, MacroExpandCallbackFn,
};
pub use cycle::{
    collect as gc_collect, env_chain_pins as gc_env_chain_pins, last_stats as gc_last_stats,
    maybe_collect as gc_maybe_collect, register_candidate, register_closure_birth,
    register_env_candidate, register_payload_tracer, registry_len as gc_registry_len,
    set_gc_observer, set_runtime_interior_hooks, should_collect as gc_should_collect,
    threshold_collect as gc_threshold_collect, trace_value, EnvBindings, GcEdge, GcNode,
    GcPassEvent, GcStats, GcTrigger, NodePtr, OpaqueSeverFn, OpaqueTraceFn, PayloadTracer,
    RuntimeInteriorHooks,
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
    clear_mcp_cassette_hook, mcp_cassette_decide, set_mcp_cassette_hook, McpCassetteDecision,
    McpCassetteRecordTarget, McpCassetteRecorder,
};
pub use mutable_ops::{mutable_array_get, mutable_array_set};
pub use output_hook::{
    capturing_root_count, current_root, mark_root_capturing, register_output_capture_sink,
    set_current_root, set_host_stderr_hook, set_host_stdout_hook, unmark_root_capturing,
    unregister_output_capture_sink, write_stderr, write_stdout, CapturedOutput,
};
pub use sandbox::{Caps, Sandbox};
pub use text_util::truncate_chars;
pub use value::{
    bits_to_spur, compare_spurs, intern, interner_stats, next_gensym, pretty_print, resolve,
    resolve_multimethod_handler, select_multimethod_handler, spur_to_bits, with_resolved, Agent,
    AsyncPromise, Channel, Conversation, Env, ImageAttachment, Lambda, Macro, Message, MultiMethod,
    MutableArray, MutableCell, NativeFn, PromiseState, Prompt, Record, Role, SemaStream, StreamBox,
    SyntaxRules, Thunk, ToolDefinition, Value, ValueView, ValueViewRef, NAN_INT_SIGN_BIT,
    NAN_INT_SMALL_PATTERN, NAN_PAYLOAD_BITS, NAN_PAYLOAD_MASK, NAN_TAG_MASK, TAG_NATIVE_FN,
};

pub mod runtime;
