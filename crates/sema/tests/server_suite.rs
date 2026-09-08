//! Grouped integration-test harness (one linked binary instead of one per file;
//! see docs/build-time-report.md). Member files live in tests/suites/. Files that
//! need process-global isolation (sema_otel::testing::install, env-var toggles)
//! stay as their own top-level test files — do NOT move them in here.

#[macro_use] // make common's macros (eval_tests! et al) visible in the member modules below
mod common;

#[path = "suites/http_concurrent_test.rs"]
mod http_concurrent_test;
#[path = "suites/http_serve_cancel_test.rs"]
mod http_serve_cancel_test;
#[path = "suites/http_serve_concurrent_test.rs"]
mod http_serve_concurrent_test;
#[path = "suites/server_async_test.rs"]
mod server_async_test;
#[path = "suites/server_test.rs"]
mod server_test;
#[path = "suites/web_dev_server_test.rs"]
mod web_dev_server_test;
