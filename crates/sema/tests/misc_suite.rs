//! Grouped integration-test harness (one linked binary instead of one per file;
//! see docs/build-time-report.md). Member files live in tests/suites/. Files that
//! need process-global isolation (sema_otel::testing::install, env-var toggles)
//! stay as their own top-level test files — do NOT move them in here.

#[macro_use] // make common's macros (eval_tests! et al) visible in the member modules below
mod common;

#[path = "suites/doc_examples_test.rs"]
mod doc_examples_test;
#[path = "suites/fmt_cli_test.rs"]
mod fmt_cli_test;
#[path = "suites/host_api_test.rs"]
mod host_api_test;
#[path = "suites/repl_display_test.rs"]
mod repl_display_test;
