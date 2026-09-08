//! End-to-end coverage for workflow-viewer approval reads and signed writes.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sema_workflow::approval::{self, ApprovalResolution, ApprovalSigningKey, NewApprovalRequest};

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sema-view-approval-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

fn seed_request(root: &Path, key: &ApprovalSigningKey) -> approval::ApprovalRequest {
    let run_dir = root.join("run-1");
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(run_dir.join("events.jsonl"), "").unwrap();
    let request = approval::ApprovalRequest::new(NewApprovalRequest {
        run_id: "run-1".into(),
        workflow: "release".into(),
        code_version: "code-a".into(),
        args_digest: "args-a".into(),
        phase: "publish".into(),
        key: "release-signoff".into(),
        occurrence: 0,
        subject_digest: "subject-a".into(),
        reason: "Publish the release".into(),
        preview: Some("Publish sema-policies@1.0.0".into()),
        requested_at: "2026-07-31T12:00:00Z".into(),
        authority_public_key: key.public_key_base64().unwrap(),
    });
    approval::ensure_request(&run_dir, &request).unwrap();
    request
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().copied())
        .finish()
}

#[tokio::test]
async fn viewer_approves_with_form_token_and_preserves_the_winning_decision() {
    sema_llm::http::ensure_crypto_provider();
    let root = temp_root("roundtrip");
    let signing_key = ApprovalSigningKey::generate().unwrap();
    let request = seed_request(&root, &signing_key);
    let authority =
        sema::workflow_view::ApprovalAuthority::new(signing_key, "viewer-default".to_string())
            .unwrap();
    let (addr, token) =
        sema::workflow_view::serve_test_with_approval(root.clone(), None, Some(authority))
            .await
            .unwrap();
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    let pending = client
        .get(format!("{base}/api/run/run-1/approvals"))
        .send()
        .await
        .unwrap();
    assert_eq!(pending.status(), reqwest::StatusCode::OK);
    let pending: serde_json::Value = pending.json().await.unwrap();
    assert_eq!(pending[0]["status"], "pending");
    assert_eq!(pending[0]["can_decide"], true);
    assert_eq!(pending[0]["default_actor"], "viewer-default");
    assert!(pending[0].get("authority_public_key").is_none());

    let endpoint = format!(
        "{base}/api/run/run-1/approvals/{}/decision",
        request.approval_id
    );
    let forbidden = client
        .post(&endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&[("decision", "approve")]))
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), reqwest::StatusCode::FORBIDDEN);

    // A native form cannot set the custom header, so the hidden per-process token is
    // accepted from the bounded body as a progressive-enhancement fallback.
    let approved = client
        .post(&endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&[
            ("view_token", token.as_str()),
            ("decision", "approve"),
            ("actor", "release manager"),
            ("comment", "verified package and version"),
        ]))
        .send()
        .await
        .unwrap();
    assert_eq!(approved.status(), reqwest::StatusCode::OK);
    let approved: serde_json::Value = approved.json().await.unwrap();
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["decision"]["actor"], "release manager");
    assert_eq!(approved["decision"]["provenance"], "web-viewer");

    let lost_race = client
        .post(&endpoint)
        .header("X-Sema-View-Token", &token)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&[
            ("decision", "reject"),
            ("actor", "second reviewer"),
            ("reason", "changed my mind"),
        ]))
        .send()
        .await
        .unwrap();
    assert_eq!(lost_race.status(), reqwest::StatusCode::CONFLICT);
    let winner: serde_json::Value = lost_race.json().await.unwrap();
    assert_eq!(winner["status"], "approved");
    assert_eq!(winner["decision"]["actor"], "release manager");

    let resolutions = approval::list_requests(&root, "run-1").unwrap();
    assert!(matches!(
        &resolutions[0],
        ApprovalResolution::Approved(_, decision)
            if decision.actor == "release manager" && decision.provenance == "web-viewer"
    ));
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn viewer_without_the_matching_authority_stays_inspect_only() {
    sema_llm::http::ensure_crypto_provider();
    let root = temp_root("wrong-key");
    let request_key = ApprovalSigningKey::generate().unwrap();
    let request = seed_request(&root, &request_key);
    let wrong_authority = sema::workflow_view::ApprovalAuthority::new(
        ApprovalSigningKey::generate().unwrap(),
        "wrong actor".to_string(),
    )
    .unwrap();
    let (addr, token) =
        sema::workflow_view::serve_test_with_approval(root.clone(), None, Some(wrong_authority))
            .await
            .unwrap();
    let client = reqwest::Client::new();
    let rows: serde_json::Value = client
        .get(format!("http://{addr}/api/run/run-1/approvals"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rows[0]["can_decide"], false);
    assert_eq!(rows[0]["decision_unavailable"], "authority-mismatch");

    let response = client
        .post(format!(
            "http://{addr}/api/run/run-1/approvals/{}/decision",
            request.approval_id
        ))
        .header("X-Sema-View-Token", token)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&[("decision", "approve")]))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    assert!(matches!(
        approval::list_requests(&root, "run-1").unwrap()[0],
        ApprovalResolution::Pending(_)
    ));
    let _ = fs::remove_dir_all(root);
}
