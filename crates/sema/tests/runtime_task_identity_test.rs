//! Runtime-scoped ownership regressions for task-backed LLM state.

#![cfg(not(target_arch = "wasm32"))]

use std::time::{Duration, Instant};

use sema_core::runtime::{CancelReason, TaskOutcome};
use sema_core::Value;
use sema_eval::Interpreter;
use sema_llm::builtins::{
    agent_runs_len, register_test_provider, reset_runtime_state, stream_runs_len,
};
use sema_llm::fake::FakeProvider;
use sema_vm::runtime::{RootOptions, RootPoll};

fn drive_pair_until_llm_runs_are_parked(left: &Interpreter, right: &Interpreter) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while agent_runs_len() != 2 || stream_runs_len() != 2 {
        left.drive_turn().expect("left runtime drive succeeds");
        right.drive_turn().expect("right runtime drive succeeds");
        assert!(
            Instant::now() < deadline,
            "both runtimes park with live agent and stream state"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn drive_until_runtime_reaped(interpreter: &Interpreter, root: &sema_vm::runtime::RootHandle) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while matches!(root.poll_result(), RootPoll::Pending)
        || interpreter.runtime_live_task_count() > 0
    {
        interpreter.drive_turn().expect("runtime drive succeeds");
        assert!(
            Instant::now() < deadline,
            "root settles and cancelled tasks are reaped before the deadline"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn reaping_one_runtime_keeps_colliding_agent_and_stream_state_owned_by_another() {
    let left = Interpreter::new();
    let right = Interpreter::new();
    reset_runtime_state();

    let chunks: Vec<String> = (0..40).map(|index| format!("chunk-{index} ")).collect();
    let chunk_refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
    let expected = chunks.concat();
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .stream(&chunk_refs)
        .stream(&chunk_refs)
        .stream_chunk_delay(10)
        .build();
    register_test_provider(Box::new(fake));

    // Each fresh runtime allocates its root main as task 1 and this spawned
    // agent driver as task 2. The local task numbers collide intentionally.
    let program = r#"
        (defagent bot {:model "fake-model" :max-turns 2})
        (async/await
          (async/spawn
            (fn ()
              (agent/run bot "go" {:on-text (fn (chunk) nil)}))))
    "#;
    let left_root = left
        .submit_str(program, RootOptions::default())
        .expect("left root admitted");
    let right_root = right
        .submit_str(program, RootOptions::default())
        .expect("right root admitted");

    drive_pair_until_llm_runs_are_parked(&left, &right);
    assert!(matches!(left_root.poll_result(), RootPoll::Pending));
    assert!(matches!(right_root.poll_result(), RootPoll::Pending));

    assert!(left_root.cancel(CancelReason::Explicit));
    drive_until_runtime_reaped(&left, &left_root);
    let RootPoll::Ready(left_settlement) = left_root.poll_result() else {
        panic!("cancelled left root settles")
    };
    assert!(matches!(
        left_settlement.outcome,
        TaskOutcome::Cancelled(CancelReason::Explicit)
    ));

    assert_eq!(
        agent_runs_len(),
        1,
        "left reaping keeps the right runtime's colliding agent state"
    );
    assert_eq!(
        stream_runs_len(),
        1,
        "left reaping keeps the right runtime's colliding stream state"
    );
    assert!(matches!(right_root.poll_result(), RootPoll::Pending));

    let right_result = right
        .drive_until_settled(&right_root)
        .expect("right runtime completes after left cancellation");
    let result_map = right_result.as_map_rc().expect("agent opts return a map");
    assert_eq!(
        result_map
            .get(&Value::keyword("response"))
            .and_then(Value::as_str),
        Some(expected.as_str())
    );
    assert_eq!(agent_runs_len(), 0);
    assert_eq!(stream_runs_len(), 0);
}
