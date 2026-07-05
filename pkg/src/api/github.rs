use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use hmac::{Hmac, Mac};
use sea_orm::*;
use serde::Deserialize;
use sha2::Sha256;
use std::sync::Arc;

use super::ApiError;
use crate::{
    auth::AuthUser,
    entity::{github_sync_log, owner, package},
    github_sync, AppState,
};

#[derive(Deserialize)]
pub struct LinkRequest {
    pub repository_url: String,
}

pub async fn link(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Json(body): Json<LinkRequest>,
) -> Result<Response, ApiError> {
    // Parse the GitHub URL
    let (owner_name, repo) =
        github_sync::parse_github_url(&body.repository_url).ok_or_else(|| {
            ApiError::bad_request("Invalid GitHub URL. Expected format: github.com/owner/repo")
        })?;

    // Get the user's GitHub token
    // (Not an ApiError: this response carries an extra "connect_url" field.)
    let token = match github_sync::get_github_token(
        &state.db,
        user.id,
        &state.config.oauth_token_key,
    )
    .await
    {
        Some(t) => t,
        None => {
            return Ok((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": "GitHub not connected",
                    "connect_url": "/auth/github?mode=connect&return_to=/account"
                })),
            )
                .into_response())
        }
    };

    let client = reqwest::Client::new();

    // Validate repo exists and has sema.toml
    let manifest = match github_sync::validate_repo(&client, &token, &owner_name, &repo).await {
        Ok(m) => m,
        Err(e) => {
            if e.contains("invalid or revoked") {
                github_sync::mark_token_revoked(&state.db, user.id).await;
            }
            return Err(ApiError::bad_request(e));
        }
    };

    // A repo's manifest name is attacker-controlled (they own the repo), so it
    // must pass the same allowlist as a CLI publish before we store it.
    crate::api::packages::validate_package_name(&manifest.name).map_err(ApiError::bad_request)?;

    // Check if package name is already taken
    let existing = package::Entity::find()
        .filter(package::Column::Name.eq(&manifest.name))
        .one(&state.db)
        .await
        .ok()
        .flatten();

    if let Some(pkg) = existing {
        return Err(ApiError::conflict(format!(
            "Package '{}' already exists (source: {})",
            manifest.name, pkg.source
        )));
    }

    // Generate webhook secret
    let webhook_secret = github_sync::generate_webhook_secret();
    let github_repo = format!("{owner_name}/{repo}");

    // Create package with source=github
    let pkg_model = package::ActiveModel {
        name: Set(manifest.name.clone()),
        description: Set(manifest.description.clone()),
        repository_url: Set(Some(format!("https://github.com/{github_repo}"))),
        source: Set("github".into()),
        github_repo: Set(Some(github_repo.clone())),
        webhook_secret: Set(Some(webhook_secret.clone())),
        ..Default::default()
    };

    let package_id = pkg_model
        .insert(&state.db)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create package: {e}")))?
        .id;

    // Add user as owner
    let owner_model = owner::ActiveModel {
        package_id: Set(package_id),
        user_id: Set(user.id),
    };
    let _ = owner_model.insert(&state.db).await;

    // Register webhook
    let webhook_url = format!("{}/api/v1/webhooks/github", state.config.base_url);
    if let Err(e) = github_sync::register_webhook(
        &client,
        &token,
        &owner_name,
        &repo,
        &webhook_url,
        &webhook_secret,
    )
    .await
    {
        tracing::warn!("Failed to register webhook for {github_repo}: {e}");
    }

    // Import existing semver tags
    let tags = github_sync::list_semver_tags(&client, &token, &owner_name, &repo)
        .await
        .unwrap_or_default();
    let mut imported = 0u32;
    let mut errors = Vec::new();

    for (tag_name, version) in &tags {
        match github_sync::sync_tag(
            &state.db,
            &owner_name,
            &repo,
            tag_name,
            version,
            package_id,
            manifest.sema_version_req.as_deref(),
        )
        .await
        {
            Ok(true) => imported += 1,
            Ok(false) => {}
            Err(e) => {
                let log_model = github_sync_log::ActiveModel {
                    package_id: Set(package_id),
                    tag: Set(tag_name.clone()),
                    status: Set("error".into()),
                    error: Set(Some(e.clone())),
                    ..Default::default()
                };
                let _ = log_model.insert(&state.db).await;
                errors.push(format!("{tag_name}: {e}"));
            }
        }
    }

    // Fetch README
    let readme_raw = github_sync::fetch_readme(&client, &token, &owner_name, &repo).await;
    if let Some(ref raw) = readme_raw {
        let html = github_sync::render_readme(raw);
        if let Ok(Some(pkg_model)) = package::Entity::find_by_id(package_id).one(&state.db).await {
            let mut pkg_active: package::ActiveModel = pkg_model.into();
            pkg_active.readme_raw = Set(Some(raw.clone()));
            pkg_active.readme_html = Set(Some(html));
            let _ = pkg_active.update(&state.db).await;
        }
    }

    crate::audit::log(
        &state.db,
        &user.username,
        "link_repo",
        Some("package"),
        Some(&manifest.name),
        Some(&github_repo),
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ok": true,
            "package": manifest.name,
            "source": "github",
            "github_repo": github_repo,
            "tags_found": tags.len(),
            "versions_imported": imported,
            "errors": errors,
        })),
    )
        .into_response())
}

pub async fn sync(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let pkg_row = state.db.query_one(Statement::from_sql_and_values(
        state.db.get_database_backend(),
        "SELECT p.id, p.source, p.github_repo FROM packages p JOIN owners o ON o.package_id = p.id WHERE p.name = ? AND o.user_id = ?",
        [name.clone().into(), user.id.into()],
    ))
    .await
    .ok()
    .flatten()
    .ok_or_else(|| ApiError::not_found("Package not found or you are not an owner"))?;

    let source: String = pkg_row.try_get("", "source").unwrap_or_default();
    if source != "github" {
        return Err(ApiError::bad_request("Package is not GitHub-linked"));
    }

    let package_id: i64 = pkg_row.try_get("", "id").unwrap_or_default();
    let github_repo: String = pkg_row.try_get("", "github_repo").unwrap_or_default();

    let (owner_name, repo) = github_sync::parse_github_url(&github_repo)
        .ok_or_else(|| ApiError::internal("Invalid github_repo in database"))?;

    let token = github_sync::get_github_token(&state.db, user.id, &state.config.oauth_token_key)
        .await
        .ok_or_else(|| {
            ApiError::forbidden("GitHub not connected. Reconnect at /auth/github?mode=connect")
        })?;

    let client = reqwest::Client::new();
    let tags = match github_sync::list_semver_tags(&client, &token, &owner_name, &repo).await {
        Ok(t) => t,
        Err(e) => {
            if e.contains("invalid or revoked") || e.contains("401") {
                github_sync::mark_token_revoked(&state.db, user.id).await;
            }
            return Err(ApiError::new(StatusCode::BAD_GATEWAY, e));
        }
    };

    let mut imported = 0u32;
    for (tag_name, version) in &tags {
        match github_sync::sync_tag(
            &state.db,
            &owner_name,
            &repo,
            tag_name,
            version,
            package_id,
            None,
        )
        .await
        {
            Ok(true) => imported += 1,
            Ok(false) => {}
            Err(e) => {
                let log_model = github_sync_log::ActiveModel {
                    package_id: Set(package_id),
                    tag: Set(tag_name.clone()),
                    status: Set("error".into()),
                    error: Set(Some(e)),
                    ..Default::default()
                };
                let _ = log_model.insert(&state.db).await;
            }
        }
    }

    // Fetch README
    let readme_raw = github_sync::fetch_readme(&client, &token, &owner_name, &repo).await;
    if let Some(ref raw) = readme_raw {
        let html = github_sync::render_readme(raw);
        if let Ok(Some(pkg_model)) = package::Entity::find_by_id(package_id).one(&state.db).await {
            let mut pkg_active: package::ActiveModel = pkg_model.into();
            pkg_active.readme_raw = Set(Some(raw.clone()));
            pkg_active.readme_html = Set(Some(html));
            let _ = pkg_active.update(&state.db).await;
        }
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "tags_found": tags.len(),
        "versions_imported": imported,
    })))
}

type HmacSha256 = Hmac<Sha256>;

pub async fn webhook(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    // Get the signature header
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| ApiError::bad_request("Missing signature"))?;

    // Parse the event type
    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if event == "ping" {
        return Ok(Json(serde_json::json!({"ok": true, "event": "ping"})));
    }
    if event != "push" {
        return Ok(Json(
            serde_json::json!({"ok": true, "event": event, "skipped": true}),
        ));
    }

    // Parse the push payload to get the ref and repo
    let payload: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("Invalid JSON"))?;

    let git_ref = payload.get("ref").and_then(|r| r.as_str()).unwrap_or("");
    // Only process tag pushes
    let tag_name = match git_ref.strip_prefix("refs/tags/") {
        Some(t) => t,
        None => {
            return Ok(Json(
                serde_json::json!({"ok": true, "skipped": "not a tag push"}),
            ))
        }
    };

    // Parse as semver (strip v prefix)
    let version_str = tag_name.strip_prefix('v').unwrap_or(tag_name);
    let version = match semver::Version::parse(version_str) {
        Ok(v) => v,
        Err(_) => {
            return Ok(Json(
                serde_json::json!({"ok": true, "skipped": "not a semver tag"}),
            ))
        }
    };

    // Get the repo full_name from the payload
    let repo_full_name = payload
        .get("repository")
        .and_then(|r| r.get("full_name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");

    if repo_full_name.is_empty() {
        return Err(ApiError::bad_request("Missing repository info"));
    }

    // Find the package by github_repo. Fold "unknown repo" into the same 403
    // as a bad signature so an unauthenticated caller can't use the status
    // code to enumerate which repos are linked.
    let pkg = package::Entity::find()
        .filter(package::Column::GithubRepo.eq(repo_full_name))
        .filter(package::Column::Source.eq("github"))
        .one(&state.db)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::forbidden("Invalid signature"))?;

    let package_id = pkg.id;

    // A missing/empty webhook secret must never validate: HMAC with an empty
    // key is attacker-computable, so treat it as a misconfigured package and
    // refuse rather than accepting a forged signature.
    let webhook_secret = pkg.webhook_secret.unwrap_or_default();
    if webhook_secret.is_empty() {
        return Err(ApiError::forbidden(
            "Package has no webhook secret configured",
        ));
    }

    // Verify HMAC signature
    let expected_sig = format!("sha256={}", compute_hmac(&webhook_secret, &body));
    if !constant_time_eq(signature.as_bytes(), expected_sig.as_bytes()) {
        return Err(ApiError::forbidden("Invalid signature"));
    }

    let (owner_name, repo) = github_sync::parse_github_url(repo_full_name)
        .ok_or_else(|| ApiError::bad_request("Invalid repo name"))?;

    match github_sync::sync_tag(
        &state.db,
        &owner_name,
        &repo,
        tag_name,
        &version,
        package_id,
        None,
    )
    .await
    {
        Ok(true) => {
            tracing::info!("Webhook: synced {repo_full_name} tag {tag_name} as {version}");
            crate::audit::log(
                &state.db,
                "system",
                "webhook_sync",
                Some("package"),
                Some(repo_full_name),
                Some(tag_name),
            )
            .await;
            Ok(Json(
                serde_json::json!({"ok": true, "version": version.to_string(), "imported": true}),
            ))
        }
        Ok(false) => Ok(Json(
            serde_json::json!({"ok": true, "version": version.to_string(), "imported": false, "reason": "already exists"}),
        )),
        Err(e) => {
            let log_model = github_sync_log::ActiveModel {
                package_id: Set(package_id),
                tag: Set(tag_name.to_string()),
                status: Set("error".into()),
                error: Set(Some(e.clone())),
                ..Default::default()
            };
            let _ = log_model.insert(&state.db).await;
            Err(ApiError::internal(e))
        }
    }
}

fn compute_hmac(secret: &str, data: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key size");
    mac.update(data);
    hex::encode(mac.finalize().into_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}
