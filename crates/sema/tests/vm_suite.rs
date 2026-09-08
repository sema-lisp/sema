//! Grouped integration-test harness (one linked binary instead of one per file;
//! see docs/build-time-report.md). Member files live in tests/suites/. Files that
//! need process-global isolation (sema_otel::testing::install, env-var toggles)
//! stay as their own top-level test files — do NOT move them in here.

#[macro_use] // make common's macros (eval_tests! et al) visible in the member modules below
mod common;

#[path = "suites/compile_result_root_test.rs"]
mod compile_result_root_test;
#[path = "suites/gc_stress_test.rs"]
mod gc_stress_test;
#[path = "suites/pio_cross_validation_test.rs"]
mod pio_cross_validation_test;
#[path = "suites/runtime_conformance_test.rs"]
mod runtime_conformance_test;
#[path = "suites/serialize_roundtrip_test.rs"]
mod serialize_roundtrip_test;
#[path = "suites/vm_async_test.rs"]
mod vm_async_test;
#[path = "suites/vm_integration_test.rs"]
mod vm_integration_test;
#[path = "suites/vm_module_test.rs"]
mod vm_module_test;
