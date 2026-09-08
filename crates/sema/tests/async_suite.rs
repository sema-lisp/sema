//! Grouped integration-test harness (one linked binary instead of one per file;
//! see docs/build-time-report.md). Member files live in tests/suites/. Files that
//! need process-global isolation (sema_otel::testing::install, env-var toggles)
//! stay as their own top-level test files — do NOT move them in here.

#[macro_use] // make common's macros (eval_tests! et al) visible in the member modules below
mod common;

#[path = "suites/agent_runtime_test.rs"]
mod agent_runtime_test;
#[path = "suites/archive_pdf_patch_async_test.rs"]
mod archive_pdf_patch_async_test;
#[path = "suites/async_awaitio_test.rs"]
mod async_awaitio_test;
#[path = "suites/cli_ctrlc_test.rs"]
mod cli_ctrlc_test;
#[path = "suites/collection_callback_async_test.rs"]
mod collection_callback_async_test;
#[path = "suites/conversation_callback_async_test.rs"]
mod conversation_callback_async_test;
#[path = "suites/dap_async_breakpoint_test.rs"]
mod dap_async_breakpoint_test;
#[path = "suites/db_async_test.rs"]
mod db_async_test;
#[path = "suites/file_async_test.rs"]
mod file_async_test;
#[path = "suites/file_runtime_test.rs"]
mod file_runtime_test;
#[path = "suites/fold_callback_async_test.rs"]
mod fold_callback_async_test;
#[path = "suites/http_callback_runtime_test.rs"]
mod http_callback_runtime_test;
#[path = "suites/io_pool_identity_test.rs"]
mod io_pool_identity_test;
#[path = "suites/key_projection_callback_async_test.rs"]
mod key_projection_callback_async_test;
#[path = "suites/kv_async_test.rs"]
mod kv_async_test;
#[path = "suites/map_callback_async_test.rs"]
mod map_callback_async_test;
#[path = "suites/map_update_callback_async_test.rs"]
mod map_update_callback_async_test;
#[path = "suites/pmap_async_test.rs"]
mod pmap_async_test;
#[path = "suites/pool_map_test.rs"]
mod pool_map_test;
#[path = "suites/predicate_callback_async_test.rs"]
mod predicate_callback_async_test;
#[path = "suites/proc_pty_async_test.rs"]
mod proc_pty_async_test;
#[path = "suites/proc_run_tty_test.rs"]
mod proc_run_tty_test;
#[path = "suites/quarantined_cpu_async_test.rs"]
mod quarantined_cpu_async_test;
#[path = "suites/runtime_external_async_test.rs"]
mod runtime_external_async_test;
#[path = "suites/runtime_external_io_test.rs"]
mod runtime_external_io_test;
#[path = "suites/runtime_task_identity_test.rs"]
mod runtime_task_identity_test;
#[path = "suites/serial_async_test.rs"]
mod serial_async_test;
#[path = "suites/shell_concurrent_test.rs"]
mod shell_concurrent_test;
#[path = "suites/sort_comparator_callback_async_test.rs"]
mod sort_comparator_callback_async_test;
#[path = "suites/stream_async_test.rs"]
mod stream_async_test;
#[path = "suites/stream_file_async_test.rs"]
mod stream_file_async_test;
#[path = "suites/true_cancel_test.rs"]
mod true_cancel_test;
#[path = "suites/unified_runtime_watchdog_test.rs"]
mod unified_runtime_watchdog_test;
#[path = "suites/wasm_async_debug_test.rs"]
mod wasm_async_debug_test;
#[path = "suites/wrapper_callback_async_test.rs"]
mod wrapper_callback_async_test;
