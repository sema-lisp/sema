//! Deterministic resume tests (slice S3).
//!
//! `--resume` re-runs the workflow body but short-circuits any agent/checkpoint leaf
//! whose content-key is already in the prior run's `memo/` dir (a file's existence is the
//! source of truth). The model is NOT called for a memoized leaf, so a fresh
//! `FakeProvider`'s `call_count()` is the unambiguous "the body ran" signal. Shared
//! harness in `workflow_common`.

mod workflow_common;
use workflow_common::{run_workflow, temp_run_dir, with_workflow_env_lock, RunOpts};

use sema_core::Value;
use sema_llm::fake::FakeProvider;
use sema_workflow::{current_for, set_workflow_scope};

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
            args_json: "{}",
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
            args_json: "{}",
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
            args_json: "{}",
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
            args_json: "{}",
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
fn changing_workflow_args_invalidates_memos_and_reruns() {
    let base = temp_run_dir("resume-args-inval");
    let marker = base.join("checkpoint-reran-for-args");
    let marker_src = sema_string(marker.to_string_lossy().as_ref());
    let src = format!(
        r#"
        (defworkflow args-invalidation-demo
          "changed args should miss resume memos"
          {{:phases ["A"]}}
          (phase "A")
          (def value
            (checkpoint :value
              (begin
                (file/write "{marker_src}" "ran")
                "ok")))
          {{:status :success :value value}})
        "#
    );

    let r1 = run_workflow(
        &src,
        fresh_fake(),
        RunOpts {
            run_id: "wf_resume_args_inval",
            run_dir: &base,
            resume: false,
            code_version: "",
            args_json: r#"{"batch":1}"#,
        },
    );
    assert_eq!(r1.result["status"], "success");
    assert!(marker.exists(), "fresh run must evaluate checkpoint");

    std::fs::remove_file(&marker).expect("remove fresh-run marker");
    let r2 = run_workflow(
        &src,
        fresh_fake(),
        RunOpts {
            run_id: "wf_resume_args_inval",
            run_dir: &base,
            resume: true,
            code_version: "",
            args_json: r#"{"batch":2}"#,
        },
    );
    assert_eq!(r2.result, r1.result);
    assert!(
        marker.exists(),
        "changed workflow args must invalidate checkpoint memo"
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
            args_json: "{}",
        },
    );
    assert_eq!(
        r2.recorder.call_count(),
        1,
        "a leaf with no memo re-runs on resume"
    );

    let _ = std::fs::remove_dir_all(&base);
}

// ── A2: safe run identity (library-level) ─────────────────────────────────────
//
// These drive `set_workflow_scope` directly (task_context = None → host scope) under the
// shared workflow env lock, exercising the identity/gating logic without a full run.

#[test]
fn two_generated_runs_in_one_process_land_in_distinct_dirs() {
    with_workflow_env_lock(|| {
        let base = temp_run_dir("gen-distinct");
        std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &base);
        // No SEMA_WORKFLOW_RUN_ID → each run's id is generated. Two in the same second
        // (indeed back to back) must still get distinct dirs (nanos + process nonce).
        let g1 = set_workflow_scope("wf", "", &Value::nil(), None).expect("first run opens");
        let id1 = current_for(None).expect("scope 1 live").run_id();
        drop(g1);
        let g2 = set_workflow_scope("wf", "", &Value::nil(), None).expect("second run opens");
        let id2 = current_for(None).expect("scope 2 live").run_id();
        drop(g2);

        assert_ne!(id1, id2, "two generated ids in one process must differ");
        assert!(base.join(&id1).is_dir(), "first run dir exists: {id1}");
        assert!(base.join(&id2).is_dir(), "second run dir exists: {id2}");
        let _ = std::fs::remove_dir_all(&base);
    });
}

#[test]
fn fresh_run_into_a_dir_with_an_existing_journal_fails() {
    with_workflow_env_lock(|| {
        let base = temp_run_dir("fresh-existing");
        // A prior run's journal already occupies this dir: a fresh run must not append to
        // (and corrupt) it — the create_new claim rejects the reuse.
        let run = base.join("wf_taken");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("events.jsonl"), "{}\n").unwrap();
        std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &base);
        std::env::set_var("SEMA_WORKFLOW_RUN_ID", "wf_taken");
        let err = match set_workflow_scope("wf", "", &Value::nil(), None) {
            Ok(_) => panic!("a fresh run into an existing journal must fail"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        let _ = std::fs::remove_dir_all(&base);
    });
}

#[test]
fn library_resume_of_a_nonexistent_run_fails() {
    with_workflow_env_lock(|| {
        let base = temp_run_dir("resume-missing");
        std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &base);
        std::env::set_var("SEMA_WORKFLOW_RUN_ID", "wf_nope");
        std::env::set_var("SEMA_WORKFLOW_RESUME", "1");
        let err = match set_workflow_scope("wf", "", &Value::nil(), None) {
            Ok(_) => panic!("resume of a run with no journal must fail"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        let _ = std::fs::remove_dir_all(&base);
    });
}

#[test]
fn library_resume_without_a_run_id_fails() {
    with_workflow_env_lock(|| {
        let base = temp_run_dir("resume-noid");
        std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &base);
        std::env::set_var("SEMA_WORKFLOW_RESUME", "1");
        // No SEMA_WORKFLOW_RUN_ID: resume has nothing to reopen.
        let err = match set_workflow_scope("wf", "", &Value::nil(), None) {
            Ok(_) => panic!("resume without an explicit run id must fail"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        let _ = std::fs::remove_dir_all(&base);
    });
}

#[test]
fn each_resume_claims_a_fresh_distinct_segment() {
    with_workflow_env_lock(|| {
        let base = temp_run_dir("resume-segments");
        let run = base.join("wf_seg");
        std::fs::create_dir_all(&run).unwrap();
        // A prior run's journal must exist for resume to be allowed.
        std::fs::write(run.join("events.jsonl"), "{}\n").unwrap();
        std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &base);
        std::env::set_var("SEMA_WORKFLOW_RUN_ID", "wf_seg");
        std::env::set_var("SEMA_WORKFLOW_RESUME", "1");

        // Two resumes of the same run claim distinct sibling segments (resume-1, resume-2)
        // via the atomic create_new claim — never the same file.
        drop(set_workflow_scope("wf", "", &Value::nil(), None).expect("first resume opens"));
        drop(set_workflow_scope("wf", "", &Value::nil(), None).expect("second resume opens"));

        let mut names: Vec<String> = std::fs::read_dir(&run)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with("events.resume-"))
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "events.resume-1.jsonl".to_string(),
                "events.resume-2.jsonl".to_string()
            ],
            "each resume must claim a distinct segment"
        );
        let _ = std::fs::remove_dir_all(&base);
    });
}
