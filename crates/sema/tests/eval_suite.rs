//! Grouped integration-test harness (one linked binary instead of one per file;
//! see docs/build-time-report.md). Member files live in tests/suites/. Files that
//! need process-global isolation (sema_otel::testing::install, env-var toggles)
//! stay as their own top-level test files — do NOT move them in here.

#[macro_use] // make common's macros (eval_tests! et al) visible in the member modules below
mod common;

#[path = "suites/eval_collections_test.rs"]
mod eval_collections_test;
#[path = "suites/eval_core_test.rs"]
mod eval_core_test;
#[path = "suites/eval_data_test.rs"]
mod eval_data_test;
#[path = "suites/eval_ergonomic_test.rs"]
mod eval_ergonomic_test;
#[path = "suites/eval_intrinsic_parity_test.rs"]
mod eval_intrinsic_parity_test;
#[path = "suites/eval_io_test.rs"]
mod eval_io_test;
#[path = "suites/eval_map_test.rs"]
mod eval_map_test;
#[path = "suites/eval_stdlib_test.rs"]
mod eval_stdlib_test;
#[path = "suites/eval_test.rs"]
mod eval_test;
#[path = "suites/eval_types_test.rs"]
mod eval_types_test;
