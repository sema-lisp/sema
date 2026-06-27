//! Deterministic resume tests (slice S3).
//!
//! `--resume` re-runs the workflow body but short-circuits any agent/checkpoint leaf
//! whose content-key is already in the prior run's `memo/` dir (a file's existence is the
//! source of truth). The model is NOT called for a memoized leaf, so a fresh
//! `FakeProvider`'s `call_count()` is the unambiguous "the body ran" signal. Shared
//! harness in `workflow_common`.

mod workflow_common;
use workflow_common::{run_workflow, temp_run_dir, RunOpts};

use sema_llm::fake::FakeProvider;

const WF: &str = r#"
    (defworkflow resume-demo
      "checkpoint + agent, memoized for resume"
      {:phases ["A"]}
      (phase "A")
      (def files (checkpoint :files (list "a" "b" "c")))
      (def summary (step "summarize the files" {:name "writer"}))
      {:status :success :files files :summary summary})
"#;

fn fresh_fake() -> FakeProvider {
    FakeProvider::builder("fake")
        .model("fake-model")
        .reply_with_usage("a concise summary", 10, 5)
        .reply_with_usage("a concise summary", 10, 5) // spare, in case it re-runs
        .build()
}

fn sema_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[test]
fn resume_skips_memoized_leaves_and_returns_the_same_result() {
    let base = temp_run_dir("resume-hit");

    let r1 = run_workflow(WF, fresh_fake(), RunOpts::fresh("wf_resume_hit", &base));
    assert_eq!(
        r1.recorder.call_count(),
        1,
        "fresh run calls the model once"
    );
    assert_eq!(r1.result["status"], "success");
    assert!(base.join("wf_resume_hit/memo").is_dir(), "memo dir written");

    // Resume (same code version): NO model call — agent + checkpoint replay from memo.
    let r2 = run_workflow(
        WF,
        fresh_fake(),
        RunOpts {
            run_id: "wf_resume_hit",
            run_dir: &base,
            resume: true,
            code_version: "",
        },
    );
    assert_eq!(
        r2.recorder.call_count(),
        0,
        "resume must not call the model"
    );
    assert_eq!(r2.result, r1.result, "resumed result is identical");

    // The resume segment re-runs the skeleton (phases, run.ended) but emits NO
    // agent.started and NO checkpoint event — both were short-circuited.
    let kinds: Vec<&str> = r2
        .events
        .iter()
        .filter_map(|e| e["event"].as_str())
        .collect();
    assert!(kinds.contains(&"run.started") && kinds.contains(&"run.ended"));
    assert!(
        !kinds.contains(&"agent.started"),
        "memoized agent re-emitted: {kinds:?}"
    );
    assert!(
        !kinds.contains(&"checkpoint"),
        "memoized checkpoint re-emitted: {kinds:?}"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn resume_is_per_leaf_checkpoint_replays_while_agent_reruns() {
    // Conservative-resume GRANULARITY: deleting only the agent's memo must re-run the
    // agent (one model call) while the checkpoint still replays (no recompute, no event).
    let base = temp_run_dir("resume-mixed");
    run_workflow(WF, fresh_fake(), RunOpts::fresh("wf_resume_mixed", &base));

    // Delete only the AGENT's memo (the sidecar whose value is the summary text); the
    // checkpoint's memo (the files list) stays.
    let memo = base.join("wf_resume_mixed/memo");
    for entry in std::fs::read_dir(&memo).unwrap().flatten() {
        let body = std::fs::read_to_string(entry.path()).unwrap_or_default();
        if body.contains("concise summary") {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }

    let r = run_workflow(
        WF,
        fresh_fake(),
        RunOpts {
            run_id: "wf_resume_mixed",
            run_dir: &base,
            resume: true,
            code_version: "",
        },
    );
    assert_eq!(
        r.recorder.call_count(),
        1,
        "agent (memo deleted) must re-run exactly once"
    );
    let kinds: Vec<&str> = r
        .events
        .iter()
        .filter_map(|e| e["event"].as_str())
        .collect();
    assert!(
        kinds.contains(&"agent.started"),
        "the re-run agent emits events"
    );
    assert!(
        !kinds.contains(&"checkpoint"),
        "the still-memoized checkpoint must NOT recompute/emit: {kinds:?}"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn resume_does_not_evaluate_memoized_checkpoint_write_expression() {
    let base = temp_run_dir("resume-lazy-checkpoint");
    let marker = base.join("checkpoint-value-ran");
    let marker_src = sema_string(marker.to_string_lossy().as_ref());
    let src = format!(
        r#"
        (defworkflow lazy-checkpoint-demo
          "checkpoint write expression should not run on memo hit"
          {{:phases ["A"]}}
          (phase "A")
          (def files
            (checkpoint :files
              (begin
                (file/write "{marker_src}" "ran")
                (list "a" "b"))))
          {{:status :success :files files}})
        "#
    );

    let r1 = run_workflow(
        &src,
        fresh_fake(),
        RunOpts::fresh("wf_resume_lazy_checkpoint", &base),
    );
    assert_eq!(r1.result["status"], "success");
    assert!(
        marker.exists(),
        "fresh run must evaluate the checkpoint write expression"
    );

    std::fs::remove_file(&marker).expect("remove fresh-run marker");
    let r2 = run_workflow(
        &src,
        fresh_fake(),
        RunOpts {
            run_id: "wf_resume_lazy_checkpoint",
            run_dir: &base,
            resume: true,
            code_version: "",
        },
    );
    assert_eq!(r2.result, r1.result);
    assert!(
        !marker.exists(),
        "memoized checkpoint write expression must not run during resume"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn checkpoint_event_content_key_points_to_memo_file() {
    let base = temp_run_dir("resume-content-key");
    let run_id = "wf_resume_content_key";
    let r = run_workflow(WF, fresh_fake(), RunOpts::fresh(run_id, &base));
    assert_eq!(r.result["status"], "success");

    let checkpoints = workflow_common::events_of(&r.events, "checkpoint");
    let content_key = checkpoints[0]["content_key"]
        .as_str()
        .expect("checkpoint event carries content_key");
    let memo_path = base
        .join(run_id)
        .join("memo")
        .join(format!("{content_key}.json"));
    assert!(
        memo_path.is_file(),
        "checkpoint content_key must point to its memo sidecar: {}",
        memo_path.display()
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn editing_the_workflow_invalidates_memos_and_reruns() {
    let base = temp_run_dir("resume-inval");
    let r1 = run_workflow(WF, fresh_fake(), RunOpts::fresh("wf_resume_inval", &base));
    assert_eq!(r1.recorder.call_count(), 1);

    // Resume with a DIFFERENT code version (= an edited workflow): all content-keys
    // change, so no memo matches and the agent re-runs.
    let r2 = run_workflow(
        WF,
        fresh_fake(),
        RunOpts {
            run_id: "wf_resume_inval",
            run_dir: &base,
            resume: true,
            code_version: "v2-edited",
        },
    );
    assert_eq!(
        r2.recorder.call_count(),
        1,
        "a changed code version re-runs the agent"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn missing_memo_reruns_conservatively() {
    let base = temp_run_dir("resume-miss");
    let r1 = run_workflow(WF, fresh_fake(), RunOpts::fresh("wf_resume_miss", &base));
    assert_eq!(r1.recorder.call_count(), 1);

    // Wipe the memo dir (an abandoned/crashed leaf with no recorded value): resume must
    // conservatively re-run rather than resume wrong.
    let _ = std::fs::remove_dir_all(base.join("wf_resume_miss/memo"));
    let r2 = run_workflow(
        WF,
        fresh_fake(),
        RunOpts {
            run_id: "wf_resume_miss",
            run_dir: &base,
            resume: true,
            code_version: "",
        },
    );
    assert_eq!(
        r2.recorder.call_count(),
        1,
        "a leaf with no memo re-runs on resume"
    );

    let _ = std::fs::remove_dir_all(&base);
}
