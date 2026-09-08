//! Grouped integration-test harness (one linked binary instead of one per file;
//! see docs/build-time-report.md). Member files live in tests/suites/. Files that
//! need process-global isolation (sema_otel::testing::install, env-var toggles)
//! stay as their own top-level test files — do NOT move them in here.

#[macro_use] // make common's macros (eval_tests! et al) visible in the member modules below
mod common;

mod workflow_common;

#[path = "suites/workflow_approval_cli_test.rs"]
mod workflow_approval_cli_test;
#[path = "suites/workflow_budget_test.rs"]
mod workflow_budget_test;
#[path = "suites/workflow_checkpoint_async_test.rs"]
mod workflow_checkpoint_async_test;
#[path = "suites/workflow_cookbook_test.rs"]
mod workflow_cookbook_test;
#[path = "suites/workflow_mcp_seam_test.rs"]
mod workflow_mcp_seam_test;
#[path = "suites/workflow_policy_test.rs"]
mod workflow_policy_test;
#[path = "suites/workflow_resume_test.rs"]
mod workflow_resume_test;
#[path = "suites/workflow_selfrewrite_test.rs"]
mod workflow_selfrewrite_test;
#[path = "suites/workflow_spike1_test.rs"]
mod workflow_spike1_test;
#[path = "suites/workflow_tools_test.rs"]
mod workflow_tools_test;
#[path = "suites/workflow_view_approval_test.rs"]
mod workflow_view_approval_test;
