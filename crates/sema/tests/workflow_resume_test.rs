//! Deterministic resume tests (track #2, slice S3).
//!
//! `--resume` re-runs the workflow body but short-circuits any agent/checkpoint leaf
//! whose content-key is already in the prior run's `memo/` dir (the file's existence is
//! the source of truth — no journal re-parsing, no frozen-vocab change). The model is
//! NOT called for a memoized leaf, so a fresh `FakeProvider`'s `call_count()` is the
//! unambiguous "the body ran" signal (there is no cassette to confound it).
//!
//! Same env-isolation discipline as the other workflow tests: own binary + `SERIAL`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use sema_eval::Interpreter;
use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::{FakeProvider, FakeRecorder};

static SERIAL: Mutex<()> = Mutex::new(());

const WF: &str = r#"
    (defworkflow resume-demo
      "checkpoint + agent, memoized for resume"
      {:phases ["A"]}
      (phase "A")
      (def files (checkpoint :files (list "a" "b" "c")))
      (def summary (agent "summarize the files" {:name "writer"}))
      {:status :success :files files :summary summary})
"#;

/// One workflow execution into `base/<run_id>/`. `resume` and `code_version` drive the
/// resume seams. Returns the recorder (for call_count) and the events of the file
/// written THIS execution (events.jsonl on a fresh run, events.resume-N.jsonl on resume).
fn exec(
    base: &PathBuf,
    run_id: &str,
    resume: bool,
    code_version: &str,
) -> (Arc<FakeRecorder>, serde_json::Value, Vec<serde_json::Value>) {
    std::env::set_var("SEMA_WORKFLOW_FIXED_TS", "0");
    std::env::set_var("SEMA_WORKFLOW_RUN_ID", run_id);
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", base);
    std::env::set_var("SEMA_WORKFLOW_CODE_VERSION", code_version);
    if resume {
        std::env::set_var("SEMA_WORKFLOW_RESUME", "1");
    } else {
        std::env::remove_var("SEMA_WORKFLOW_RESUME");
    }

    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply_with_usage("a concise summary", 10, 5)
        .reply_with_usage("a concise summary", 10, 5) // spare, in case it re-runs
        .build();
    let recorder = fake.recorder();

    let interp = Interpreter::new();
    reset_runtime_state();
    register_test_provider(Box::new(fake));
    interp.eval_str_compiled(WF).expect("workflow evaluates");

    for v in [
        "SEMA_WORKFLOW_FIXED_TS",
        "SEMA_WORKFLOW_RUN_ID",
        "SEMA_WORKFLOW_RUN_DIR",
        "SEMA_WORKFLOW_CODE_VERSION",
        "SEMA_WORKFLOW_RESUME",
    ] {
        std::env::remove_var(v);
    }

    let run = base.join(run_id);
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("result.json")).unwrap()).unwrap();
    // The events file written THIS execution.
    let events_file = if resume {
        run.join("events.resume-1.jsonl")
    } else {
        run.join("events.jsonl")
    };
    let events = std::fs::read_to_string(&events_file)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (recorder, result, events)
}

fn tmp_base(tag: &str) -> PathBuf {
    let mut d = std::env::temp_dir();
    d.push(format!("sema-wf-resume-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn resume_skips_memoized_leaves_and_returns_the_same_result() {
    let _g: MutexGuard<()> = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let base = tmp_base("hit");

    // Fresh run: the agent calls the model exactly once; both leaves get memoized.
    let (rec1, result1, _) = exec(&base, "wf_resume_hit", false, "v1");
    assert_eq!(rec1.call_count(), 1, "fresh run calls the model once");
    assert_eq!(result1["status"], "success");
    assert!(base.join("wf_resume_hit/memo").is_dir(), "memo dir written");

    // Resume (same code version): NO model call — the agent + checkpoint replay from
    // memo, and the result is identical to the first run.
    let (rec2, result2, ev2) = exec(&base, "wf_resume_hit", true, "v1");
    assert_eq!(rec2.call_count(), 0, "resume must not call the model");
    assert_eq!(result2, result1, "resumed result is identical");

    // The resume segment re-runs the deterministic skeleton (phase markers, run.ended)
    // but emits NO agent.started and NO checkpoint event — both were short-circuited.
    let kinds: Vec<&str> = ev2.iter().filter_map(|e| e["event"].as_str()).collect();
    assert!(kinds.contains(&"run.started") && kinds.contains(&"run.ended"));
    assert!(
        !kinds.contains(&"agent.started"),
        "memoized agent must not re-emit events: {kinds:?}"
    );
    assert!(
        !kinds.contains(&"checkpoint"),
        "memoized checkpoint must not re-emit events: {kinds:?}"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn editing_the_workflow_invalidates_memos_and_reruns() {
    let _g: MutexGuard<()> = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let base = tmp_base("inval");

    let (rec1, _, _) = exec(&base, "wf_resume_inval", false, "v1");
    assert_eq!(rec1.call_count(), 1);

    // Resume with a DIFFERENT code version (= an edited workflow): the content-keys all
    // change, so no memo matches and the agent re-runs (full re-execution).
    let (rec2, _, _) = exec(&base, "wf_resume_inval", true, "v2-edited");
    assert_eq!(
        rec2.call_count(),
        1,
        "a changed code version invalidates memos → the agent re-runs"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn missing_memo_reruns_conservatively() {
    let _g: MutexGuard<()> = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let base = tmp_base("miss");

    let (rec1, _, _) = exec(&base, "wf_resume_miss", false, "v1");
    assert_eq!(rec1.call_count(), 1);

    // Wipe the memo dir (simulating an abandoned/crashed leaf with no recorded value):
    // resume must conservatively re-run rather than resume wrong.
    let _ = std::fs::remove_dir_all(base.join("wf_resume_miss/memo"));
    let (rec2, _, _) = exec(&base, "wf_resume_miss", true, "v1");
    assert_eq!(
        rec2.call_count(),
        1,
        "a leaf with no memo re-runs on resume (conservative)"
    );

    let _ = std::fs::remove_dir_all(&base);
}
