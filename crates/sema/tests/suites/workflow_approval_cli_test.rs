//! End-to-end tests for durable explicit workflow approvals at the CLI boundary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NONCE: AtomicU64 = AtomicU64::new(0);

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sema-workflow-approval-{label}-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn sema(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(args)
        .env("CI", "1")
        .output()
        .expect("run sema subprocess")
}

fn request_id(root: &Path, run_id: &str) -> String {
    let approval_dir = root.join(run_id).join("approvals");
    let request_path = fs::read_dir(&approval_dir)
        .expect("approval directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".request.json"))
        })
        .expect("request sidecar");
    serde_json::from_slice::<serde_json::Value>(&fs::read(request_path).unwrap()).unwrap()
        ["approval_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn request_ids(root: &Path, run_id: &str) -> Vec<String> {
    let mut ids: Vec<String> = fs::read_dir(root.join(run_id).join("approvals"))
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            path.file_name()
                .and_then(|name| name.to_str())
                .filter(|name| name.ends_with(".request.json"))?;
            let json: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            json["approval_id"].as_str().map(str::to_string)
        })
        .collect();
    ids.sort();
    ids
}

fn approval_keys(root: &Path) -> (PathBuf, PathBuf) {
    fs::create_dir_all(root).unwrap();
    let private = root.join("approval.private");
    let public = root.join("approval.public");
    let output = sema(&[
        "workflow",
        "approval-keygen",
        "--private-key-file",
        private.to_str().unwrap(),
        "--public-key-file",
        public.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
    (private, public)
}

fn write_workflow(root: &Path, run_id: &str) -> (PathBuf, PathBuf, PathBuf) {
    fs::create_dir_all(root).unwrap();
    let workflow = root.join("approval.sema");
    let crossed = root.join("crossed.txt");
    let bypassed = root.join("bypassed.txt");
    let path = |path: &Path| path.to_string_lossy().replace('\\', "\\\\");
    fs::write(
        &workflow,
        format!(
            r#"
            (defpolicy human-reviewed
              {{:completion {{:require-events [:approval.applied]}}}})
            (defworkflow approval-demo "approval test"
              {{:phases ["Release"] :policy human-reviewed}}
              (phase "Release")
              (checkpoint :prepared 1)
              (approval :release-signoff
                {{:reason "Publish the release"
                  :subject {{:kind :external-action :secret "do-not-store-raw"}}
                  :preview "Publish package@1.0.0"}})
              (file/write "{}" "crossed")
              {{:status :success}})
            "#,
            path(&crossed),
        ),
    )
    .unwrap();
    assert!(!root.join(run_id).exists());
    (workflow, crossed, bypassed)
}

#[test]
fn pending_approval_stops_then_approve_and_resume_crosses_gate() {
    let root = temp_root("approve");
    let run_id = "wf_approval_approve";
    let (workflow, crossed, bypassed) = write_workflow(&root, run_id);
    let (private_key, public_key) = approval_keys(&root);
    let root_s = root.to_string_lossy().to_string();
    let workflow_s = workflow.to_string_lossy().to_string();

    let pending = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "workflow",
            "run",
            &workflow_s,
            "--run-dir",
            &root_s,
            "--approval-mode",
            "pause",
            "--approval-public-key-file",
            public_key.to_str().unwrap(),
        ])
        .env("CI", "1")
        .env("SEMA_WORKFLOW_RUN_ID", run_id)
        .output()
        .unwrap();
    assert_eq!(pending.status.code(), Some(3), "{pending:?}");
    assert!(!crossed.exists(), "later side effect ran before approval");
    assert!(!bypassed.exists(), "try/catch bypassed the approval stop");
    let first_result: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(run_id).join("result.json")).unwrap()).unwrap();
    assert_eq!(first_result["status"], "needs-approval");
    assert_eq!(first_result["run-id"], run_id);

    let approval_id = request_id(&root, run_id);
    let request_text = fs::read_to_string(
        root.join(run_id)
            .join("approvals")
            .join(format!("{approval_id}.request.json")),
    )
    .unwrap();
    assert!(!request_text.contains("do-not-store-raw"));
    assert!(request_text.contains("Publish package@1.0.0"));

    let approved = sema(&[
        "workflow",
        "approve",
        run_id,
        &approval_id,
        "--run-dir",
        &root_s,
        "--actor",
        "test-operator",
        "--comment",
        "verified",
        "--signing-key-file",
        private_key.to_str().unwrap(),
    ]);
    assert!(approved.status.success(), "{approved:?}");

    let resumed = sema(&[
        "workflow",
        "run",
        &workflow_s,
        "--run-dir",
        &root_s,
        "--resume",
        run_id,
        "--approval-mode",
        "pause",
        "--approval-public-key-file",
        public_key.to_str().unwrap(),
    ]);
    assert!(resumed.status.success(), "{resumed:?}");
    assert_eq!(fs::read_to_string(&crossed).unwrap(), "crossed");
    assert!(!bypassed.exists());

    let events = fs::read_to_string(root.join(run_id).join("events.resume-1.jsonl")).unwrap();
    assert!(events.contains(r#""event":"approval.granted""#));
    assert!(events.contains(r#""event":"approval.applied""#));

    let listed = sema(&[
        "workflow",
        "approvals",
        run_id,
        "--run-dir",
        &root_s,
        "--json",
    ]);
    assert!(listed.status.success(), "{listed:?}");
    let list: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(list[0]["status"], "approved");
    assert_eq!(list[0]["decision"]["actor"], "test-operator");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn approval_in_parallel_is_rejected_before_execution() {
    let root = temp_root("parallel");
    fs::create_dir_all(&root).unwrap();
    let run_id = "wf_approval_parallel";
    let workflow = root.join("parallel-approval.sema");
    let crossed = root.join("parallel-crossed.txt");
    let crossed_literal = serde_json::to_string(&crossed.to_string_lossy()).unwrap();
    fs::write(
        &workflow,
        format!(
            r#"
            (defworkflow approval-parallel "parallel approval test" {{:phases ["Release"]}}
              (phase "Release")
              (parallel
                (list
                  (fn ()
                    (approval :parallel-signoff
                      {{:reason "Approve the parallel release"
                        :subject {{:kind :release :target "production"}}}}))))
              (file/write {crossed_literal} "crossed")
              {{:status :success}})
            "#
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "workflow",
            "run",
            &workflow.to_string_lossy(),
            "--run-dir",
            &root.to_string_lossy(),
            "--approval-mode",
            "pause",
        ])
        .env("CI", "1")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("E-APPROVAL-PLACEMENT"),
        "{output:?}"
    );
    assert!(
        !crossed.exists(),
        "a later workflow form ran after a parallel approval gate"
    );
    assert!(!root.join(run_id).exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn rejected_approval_resumes_to_rejected_without_crossing_gate() {
    let root = temp_root("reject");
    let run_id = "wf_approval_reject";
    let (workflow, crossed, bypassed) = write_workflow(&root, run_id);
    let (private_key, public_key) = approval_keys(&root);
    let root_s = root.to_string_lossy().to_string();
    let workflow_s = workflow.to_string_lossy().to_string();
    let pending = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "workflow",
            "run",
            &workflow_s,
            "--run-dir",
            &root_s,
            "--approval-mode",
            "pause",
            "--approval-public-key-file",
            public_key.to_str().unwrap(),
        ])
        .env("CI", "1")
        .env("SEMA_WORKFLOW_RUN_ID", run_id)
        .output()
        .unwrap();
    assert_eq!(pending.status.code(), Some(3));
    let approval_id = request_id(&root, run_id);

    let rejected = sema(&[
        "workflow",
        "reject",
        run_id,
        &approval_id,
        "--run-dir",
        &root_s,
        "--actor",
        "reviewer",
        "--reason",
        "release is not ready",
        "--signing-key-file",
        private_key.to_str().unwrap(),
    ]);
    assert!(rejected.status.success(), "{rejected:?}");
    let resumed = sema(&[
        "workflow",
        "run",
        &workflow_s,
        "--run-dir",
        &root_s,
        "--resume",
        run_id,
        "--approval-mode",
        "pause",
        "--approval-public-key-file",
        public_key.to_str().unwrap(),
    ]);
    assert_eq!(resumed.status.code(), Some(1), "{resumed:?}");
    assert!(!crossed.exists());
    assert!(!bypassed.exists(), "try/catch bypassed a rejection");
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(run_id).join("result.json")).unwrap()).unwrap();
    assert_eq!(result["status"], "rejected");
    assert_eq!(result["reason"], "release is not ready");
    let events = fs::read_to_string(root.join(run_id).join("events.resume-1.jsonl")).unwrap();
    assert!(events.contains(r#""event":"approval.rejected""#));
    assert!(!events.contains(r#""event":"approval.applied""#));

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn terminal_auto_mode_prompts_approves_and_resumes_inline() {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let root = temp_root("terminal-prompt");
    let run_id = "wf_approval_terminal";
    let (workflow, crossed, bypassed) = write_workflow(&root, run_id);
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open approval prompt pty");
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_sema"));
    command.args([
        "workflow",
        "run",
        &workflow.to_string_lossy(),
        "--run-dir",
        &root.to_string_lossy(),
        "--approval-mode",
        "auto",
    ]);
    command.env("CI", "");
    command.env("TERM", "xterm-256color");
    command.env("SEMA_WORKFLOW_RUN_ID", run_id);
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn approval prompt");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("clone pty reader");
    let mut writer = pair.master.take_writer().expect("take pty writer");
    let (send_chunk, receive_chunk) = mpsc::channel();
    let reader_thread = std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) if send_chunk.send(chunk[..read].to_vec()).is_err() => break,
                Ok(_) => {}
            }
        }
    });

    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !String::from_utf8_lossy(&output).contains("Approve?") {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let _ = child.kill();
            panic!(
                "approval prompt did not appear: {}",
                String::from_utf8_lossy(&output)
            );
        }
        output.extend(
            receive_chunk
                .recv_timeout(remaining)
                .unwrap_or_else(|error| panic!("read approval prompt: {error}")),
        );
    }
    writer.write_all(b"y\n").expect("approve in terminal");
    writer.flush().expect("flush terminal approval");
    let status = child.wait().expect("wait for approval workflow");
    drop(writer);
    let _ = reader_thread.join();

    assert!(
        status.success(),
        "interactive workflow failed: {}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(fs::read_to_string(crossed).unwrap(), "crossed");
    assert!(!bypassed.exists());
    let approval_id = request_id(&root, run_id);
    let decision: serde_json::Value = serde_json::from_slice(
        &fs::read(
            root.join(run_id)
                .join("approvals")
                .join(format!("{approval_id}.decision.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(decision["decision"], "approve");
    assert_eq!(decision["provenance"], "terminal-prompt");

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn ctrl_c_at_terminal_prompt_exits_and_leaves_request_pending() {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::Read;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let root = temp_root("terminal-ctrlc");
    let run_id = "wf_approval_terminal_ctrlc";
    let (workflow, crossed, bypassed) = write_workflow(&root, run_id);
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open approval prompt pty");
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_sema"));
    command.args([
        "workflow",
        "run",
        &workflow.to_string_lossy(),
        "--run-dir",
        &root.to_string_lossy(),
        "--approval-mode",
        "auto",
    ]);
    command.env("CI", "");
    command.env("TERM", "xterm-256color");
    command.env("SEMA_WORKFLOW_RUN_ID", run_id);
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn approval prompt");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("clone pty reader");
    let (send_chunk, receive_chunk) = mpsc::channel();
    let reader_thread = std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) if send_chunk.send(chunk[..read].to_vec()).is_err() => break,
                Ok(_) => {}
            }
        }
    });

    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !String::from_utf8_lossy(&output).contains("Approve?") {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let _ = child.kill();
            panic!(
                "approval prompt did not appear: {}",
                String::from_utf8_lossy(&output)
            );
        }
        output.extend(
            receive_chunk
                .recv_timeout(remaining)
                .unwrap_or_else(|error| panic!("read approval prompt: {error}")),
        );
    }

    let pid = child.process_id().expect("approval process id");
    let kill_result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) };
    assert_eq!(kill_result, 0, "send SIGINT to approval process");
    let status = child
        .wait()
        .expect("wait for interrupted approval workflow");
    drop(pair.master);
    let _ = reader_thread.join();

    assert_eq!(
        status.exit_code(),
        130,
        "unexpected interrupted output: {}",
        String::from_utf8_lossy(&output)
    );
    assert!(!crossed.exists(), "later side effect ran after Ctrl-C");
    assert!(!bypassed.exists(), "approval stop was caught after Ctrl-C");
    let approval_id = request_id(&root, run_id);
    assert!(
        !root
            .join(run_id)
            .join("approvals")
            .join(format!("{approval_id}.decision.json"))
            .exists(),
        "Ctrl-C created an approval decision"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn imported_code_change_requires_a_new_approval() {
    let root = temp_root("dependency-change");
    fs::create_dir_all(&root).unwrap();
    let helper = root.join("approval-helper.sema");
    fs::write(&helper, "(define release-target \"v1\")\n").unwrap();
    let workflow = root.join("dependency-approval.sema");
    let crossed = root.join("dependency-crossed.txt");
    fs::write(
        &workflow,
        format!(
            r#"(import {})
               (defworkflow dependency-approval "dependency binding" {{}}
                 (approval :release
                   {{:reason "Release imported target" :subject release-target}})
                 (file/write {} "crossed")
                 {{:status :success}})"#,
            serde_json::to_string(&helper.to_string_lossy()).unwrap(),
            serde_json::to_string(&crossed.to_string_lossy()).unwrap(),
        ),
    )
    .unwrap();
    let (private_key, public_key) = approval_keys(&root);
    let root_s = root.to_string_lossy();
    let workflow_s = workflow.to_string_lossy();

    let pending = sema(&[
        "workflow",
        "run",
        &workflow_s,
        "--run-dir",
        &root_s,
        "--approval-mode",
        "pause",
        "--approval-public-key-file",
        public_key.to_str().unwrap(),
    ]);
    assert_eq!(pending.status.code(), Some(3), "{pending:?}");
    let actual_run_id = run_id_for_root(&root);
    let first = request_id(&root, &actual_run_id);
    let approved = sema(&[
        "workflow",
        "approve",
        &actual_run_id,
        &first,
        "--run-dir",
        &root_s,
        "--signing-key-file",
        private_key.to_str().unwrap(),
    ]);
    assert!(approved.status.success(), "{approved:?}");

    fs::write(&helper, "(define release-target \"v2\")\n").unwrap();
    let resumed = sema(&[
        "workflow",
        "run",
        &workflow_s,
        "--run-dir",
        &root_s,
        "--resume",
        &actual_run_id,
        "--approval-mode",
        "pause",
        "--approval-public-key-file",
        public_key.to_str().unwrap(),
    ]);
    assert_eq!(resumed.status.code(), Some(3), "{resumed:?}");
    assert_eq!(request_ids(&root, &actual_run_id).len(), 2);
    assert!(!crossed.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn approval_run_evaluates_the_exact_import_bytes_that_were_hashed() {
    let root = temp_root("dependency-snapshot");
    fs::create_dir_all(&root).unwrap();
    let helper = root.join("snapshot-helper.sema");
    let marker = root.join("snapshot-marker.txt");
    let helper_v1 = format!(
        "(file/write {} \"v1\")\n(define release-target \"v1\")\n",
        serde_json::to_string(&marker.to_string_lossy()).unwrap()
    );
    let helper_v2 = format!(
        "(file/write {} \"v2\")\n(define release-target \"v2\")\n",
        serde_json::to_string(&marker.to_string_lossy()).unwrap()
    );
    fs::write(&helper, &helper_v1).unwrap();

    let workflow = root.join("snapshot-approval.sema");
    fs::write(
        &workflow,
        format!(
            r#"(file/write {} {})
               (import {})
               (defworkflow dependency-snapshot "dependency snapshot" {{}}
                 (approval :release
                   {{:reason "Release the snapshotted target" :subject release-target}})
                 {{:status :success}})"#,
            serde_json::to_string(&helper.to_string_lossy()).unwrap(),
            serde_json::to_string(&helper_v2).unwrap(),
            serde_json::to_string(&helper.to_string_lossy()).unwrap(),
        ),
    )
    .unwrap();
    let (_private_key, public_key) = approval_keys(&root);

    let pending = sema(&[
        "workflow",
        "run",
        &workflow.to_string_lossy(),
        "--run-dir",
        &root.to_string_lossy(),
        "--approval-mode",
        "pause",
        "--approval-public-key-file",
        public_key.to_str().unwrap(),
    ]);
    assert_eq!(pending.status.code(), Some(3), "{pending:?}");
    assert_eq!(fs::read_to_string(&helper).unwrap(), helper_v2);
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        "v1",
        "the runtime import escaped the dependency snapshot"
    );

    let _ = fs::remove_dir_all(root);
}

fn run_id_for_root(root: &Path) -> String {
    fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().join("events.jsonl").is_file())
        .expect("workflow run directory")
        .file_name()
        .to_str()
        .expect("utf-8 run id")
        .to_string()
}

#[test]
fn a_trailing_top_level_form_is_rejected_without_execution() {
    let root = temp_root("trailing-form");
    fs::create_dir_all(&root).unwrap();
    let workflow = root.join("trailing.sema");
    let crossed = root.join("trailing-crossed.txt");
    fs::write(
        &workflow,
        format!(
            "(defworkflow d \"d\" {{}} {{:status :success}})\n(file/write {} \"crossed\")\n",
            serde_json::to_string(&crossed.to_string_lossy()).unwrap()
        ),
    )
    .unwrap();
    let output = sema(&[
        "workflow",
        "run",
        workflow.to_str().unwrap(),
        "--run-dir",
        root.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("E-WF-FINAL"));
    assert!(!crossed.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn nested_and_detached_approval_aliases_are_rejected_before_execution() {
    for (label, body) in [
        (
            "nested",
            r#"(workflow/run "inner" "inner" {} (fn ()
                   (approval-gate :inner {:reason "nested" :subject 1})))"#,
        ),
        (
            "detached",
            r#"(async/spawn (fn ()
                   (approval-gate :child {:reason "detached" :subject 1})
                   (file/write protected-path "crossed")))"#,
        ),
    ] {
        let root = temp_root(label);
        fs::create_dir_all(&root).unwrap();
        let workflow = root.join(format!("{label}.sema"));
        let crossed = root.join(format!("{label}-crossed.txt"));
        fs::write(
            &workflow,
            format!(
                r#"(define approval-gate workflow/approval)
                   (define protected-path {})
                   (defworkflow outer "outer" {{}}
                     {body}
                     (file/write protected-path "outer-crossed")
                     {{:status :success}})"#,
                serde_json::to_string(&crossed.to_string_lossy()).unwrap(),
            ),
        )
        .unwrap();
        let (_private_key, public_key) = approval_keys(&root);
        let output = sema(&[
            "workflow",
            "run",
            workflow.to_str().unwrap(),
            "--run-dir",
            root.to_str().unwrap(),
            "--approval-mode",
            "pause",
            "--approval-public-key-file",
            public_key.to_str().unwrap(),
        ]);
        assert_eq!(output.status.code(), Some(1), "{label}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("E-APPROVAL-VALUE"),
            "{label}: {output:?}"
        );
        assert!(!crossed.exists(), "{label}: protected side effect ran");
        let _ = fs::remove_dir_all(root);
    }
}
