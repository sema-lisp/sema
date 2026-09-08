//! Grouped integration-test harness (one linked binary instead of one per file;
//! see docs/build-time-report.md). Member files live in tests/suites/. Files that
//! need process-global isolation (sema_otel::testing::install, env-var toggles)
//! stay as their own top-level test files — do NOT move them in here.

#[macro_use] // make common's macros (eval_tests! et al) visible in the member modules below
mod common;

#[path = "suites/mcp_async_test.rs"]
mod mcp_async_test;
#[path = "suites/mcp_builtin_test.rs"]
mod mcp_builtin_test;
#[path = "suites/mcp_cassette_test.rs"]
mod mcp_cassette_test;
#[path = "suites/mcp_e2e_test.rs"]
mod mcp_e2e_test;
#[path = "suites/mcp_runtime_test.rs"]
mod mcp_runtime_test;
