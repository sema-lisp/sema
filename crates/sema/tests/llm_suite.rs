//! Grouped integration-test harness (one linked binary instead of one per file;
//! see docs/build-time-report.md). Member files live in tests/suites/. Files that
//! need process-global isolation (sema_otel::testing::install, env-var toggles)
//! stay as their own top-level test files — do NOT move them in here.

#[macro_use] // make common's macros (eval_tests! et al) visible in the member modules below
mod common;

#[path = "suites/embedding_api_test.rs"]
mod embedding_api_test;
#[path = "suites/llm_cassette_test.rs"]
mod llm_cassette_test;
#[path = "suites/llm_chat_tools_async_test.rs"]
mod llm_chat_tools_async_test;
#[path = "suites/llm_fake_test.rs"]
mod llm_fake_test;
#[path = "suites/llm_root_nonblocking_test.rs"]
mod llm_root_nonblocking_test;
#[path = "suites/memory_test.rs"]
mod memory_test;
