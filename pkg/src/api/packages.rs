use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect},
    Json,
};
use sea_orm::TransactionTrait;
use serde::Deserialize;
use std::sync::Arc;

use super::ApiError;
use crate::{auth::TokenUser, blob, dal, AppState};

/// Magic bytes every gzip stream starts with (RFC 1952).
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Validate a published package name against a strict allowlist.
///
/// Names surface in HTML/JS contexts (the package page interpolates the name
/// into Alpine `x-init`/`@click` JavaScript), so anything outside
/// `[A-Za-z0-9._-]` — quotes, parens, angle brackets, semicolons, spaces — is
/// rejected here to prevent a stored-XSS breakout. Also blocks `..` so the
/// name can never be mistaken for a path traversal in derived filenames.
pub fn validate_package_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("Package name must be 1-64 characters".into());
    }
    if name.contains("..") {
        return Err("Package name cannot contain '..'".into());
    }
    let bytes = name.as_bytes();
    let is_alnum = |b: u8| b.is_ascii_alphanumeric();
    if !is_alnum(bytes[0]) || !is_alnum(bytes[bytes.len() - 1]) {
        return Err("Package name must start and end with a letter or digit".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("Package name may only contain letters, digits, '-', '_', and '.'".into());
    }
    Ok(())
}

// ── Publish ──

#[derive(Deserialize, Default)]
struct PublishMetadata {
    #[serde(default)]
    description: String,
    #[serde(default)]
    repository_url: Option<String>,
    #[serde(default)]
    sema_version_req: Option<String>,
    #[serde(default)]
    dependencies: Vec<DepEntry>,
}

#[derive(Deserialize)]
struct DepEntry {
    name: String,
    version_req: String,
}

pub async fn publish(
    State(state): State<Arc<AppState>>,
    TokenUser { user, scopes }: TokenUser,
    Path((name, version)): Path<(String, String)>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    if !scopes.contains("publish") {
        return Err(ApiError::forbidden("Token lacks publish scope"));
    }

    validate_package_name(&name).map_err(ApiError::bad_request)?;

    let ver = semver::Version::parse(&version)
        .map_err(|_| ApiError::bad_request("Invalid semver version"))?;

    let mut tarball_data: Option<Vec<u8>> = None;
    let mut metadata = PublishMetadata::default();

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(_) => return Err(ApiError::bad_request("Malformed multipart body")),
        };
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "tarball" => {
                let data = field
                    .bytes()
                    .await
                    .map_err(|_| ApiError::bad_request("Failed to read tarball field"))?
                    .to_vec();
                if data.len() > state.config.max_tarball_bytes {
                    return Err(ApiError::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "Tarball too large",
                    ));
                }
                tarball_data = Some(data);
            }
            "metadata" => {
                let text = field
                    .text()
                    .await
                    .map_err(|_| ApiError::bad_request("Failed to read metadata field"))?;
                metadata = serde_json::from_str::<PublishMetadata>(&text)
                    .map_err(|e| ApiError::bad_request(format!("Invalid metadata JSON: {e}")))?;
            }
            _ => {}
        }
    }

    let tarball = match tarball_data {
        Some(d) if !d.is_empty() => d,
        _ => return Err(ApiError::bad_request("Missing tarball")),
    };

    if tarball.len() < 2 || tarball[..2] != GZIP_MAGIC {
        return Err(ApiError::bad_request(
            "Tarball is not a gzip stream (bad magic bytes)",
        ));
    }

    if metadata.dependencies.len() > state.config.max_dependencies {
        return Err(ApiError::bad_request(format!(
            "Too many dependencies (max {})",
            state.config.max_dependencies
        )));
    }

    // Repository URL, if given, must be an http(s) link — it is rendered as an
    // `<a href>` on the package page, so a `javascript:`/`data:` scheme would
    // be a stored-XSS vector.
    if let Some(url) = &metadata.repository_url {
        let is_http = url.starts_with("https://") || url.starts_with("http://");
        if !url.is_empty() && !is_http {
            return Err(ApiError::bad_request(
                "repository_url must start with http:// or https://",
            ));
        }
    }

    for dep in &metadata.dependencies {
        if dep.name.is_empty() {
            return Err(ApiError::bad_request("Dependency name cannot be empty"));
        }
        if semver::VersionReq::parse(&dep.version_req).is_err() {
            return Err(ApiError::bad_request(format!(
                "Invalid version requirement '{}' for dependency '{}'",
                dep.version_req, dep.name
            )));
        }
    }

    // Ownership / source checks (reads; the writes below run in one transaction)
    let existing = dal::packages::find_by_name(&state.db, &name)
        .await
        .map_err(|_| ApiError::internal("Database error"))?;

    if let Some(pkg) = &existing {
        if pkg.source == "github" {
            return Err(ApiError::forbidden(
                "This package is GitHub-linked and cannot be published via CLI. Push a new semver tag to the linked repository instead.",
            ));
        }

        let is_owner = dal::owners::is_owner(&state.db, pkg.id, user.id)
            .await
            .unwrap_or(false);

        if !is_owner {
            return Err(ApiError::forbidden("You are not an owner of this package"));
        }
    }

    let version_str = ver.to_string();
    if let Some(pkg) = &existing {
        let exists = dal::versions::exists(&state.db, pkg.id, &version_str)
            .await
            .unwrap_or(false);

        if exists {
            return Err(ApiError::conflict("Version already exists"));
        }
    }

    // Store blob before the transaction: content-addressed, so a failure below
    // leaves at worst one orphaned file that a retried publish reuses.
    let (blob_key, checksum, size) = blob::store(&state.config.blob_dir, &tarball)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to store tarball: {e}")))?;

    // All DB writes are atomic: on any failure nothing commits (the
    // transaction rolls back on drop), so a version row can never exist
    // with missing dependency rows.
    let txn = state
        .db
        .begin()
        .await
        .map_err(|_| ApiError::internal("Failed to begin transaction"))?;

    let package_id = match &existing {
        Some(pkg) => pkg.id,
        None => {
            let pkg = dal::packages::create(
                &txn,
                &name,
                &metadata.description,
                metadata.repository_url.clone(),
            )
            .await
            .map_err(|_| ApiError::internal("Failed to create package"))?;

            dal::owners::add(&txn, pkg.id, user.id)
                .await
                .map_err(|_| ApiError::internal("Failed to create package owner"))?;

            pkg.id
        }
    };

    let version_model = dal::versions::create(
        &txn,
        package_id,
        &version_str,
        &checksum,
        blob_key,
        size as i64,
        metadata.sema_version_req.clone(),
    )
    .await
    .map_err(|_| ApiError::internal("Failed to insert version"))?;

    for dep in &metadata.dependencies {
        dal::deps::insert(&txn, version_model.id, &dep.name, &dep.version_req)
            .await
            .map_err(|_| ApiError::internal("Failed to insert dependency"))?;
    }

    // Refresh the description on existing packages (new ones got it at insert)
    if existing.is_some() && !metadata.description.is_empty() {
        dal::packages::update_description(&txn, package_id, &metadata.description)
            .await
            .map_err(|_| ApiError::internal("Failed to update package description"))?;
    }

    txn.commit()
        .await
        .map_err(|_| ApiError::internal("Failed to commit transaction"))?;

    crate::audit::log(
        &state.db,
        &user.username,
        "publish",
        Some("package"),
        Some(&name),
        Some(&version_str),
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ok": true,
            "package": name,
            "version": version_str,
            "checksum": checksum,
            "size": size,
        })),
    ))
}

// ── Get Package ──

pub async fn get_package(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let pkg = dal::packages::find_by_name(&state.db, &name)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::not_found("Package not found"))?;

    let pkg_id = pkg.id;

    let versions = dal::versions::list_for_package(&state.db, pkg_id)
        .await
        .unwrap_or_default();

    let version_list: Vec<serde_json::Value> = versions
        .iter()
        .map(|v| {
            serde_json::json!({
                "version": v.version,
                "checksum_sha256": v.checksum_sha256,
                "size_bytes": v.size_bytes,
                "yanked": v.yanked != 0,
                "sema_version_req": v.sema_version_req,
                "tarball_url": v.tarball_url,
                "published_at": v.published_at,
            })
        })
        .collect();

    let owners = dal::owners::list_usernames(&state.db, pkg_id)
        .await
        .unwrap_or_default();

    let dl_count = dal::downloads::total(&state.db, &name).await.unwrap_or(0);

    Ok(Json(serde_json::json!({
        "package": {
            "name": pkg.name,
            "description": pkg.description,
            "repository_url": pkg.repository_url,
            "created_at": pkg.created_at,
            "readme_html": pkg.readme_html,
        },
        "versions": version_list,
        "owners": owners,
        "total_downloads": dl_count,
    })))
}

// ── Download ──

pub async fn download(
    State(state): State<Arc<AppState>>,
    Path((name, version)): Path<(String, String)>,
) -> Result<axum::response::Response, ApiError> {
    let target = dal::versions::download_target(&state.db, &name, &version)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::not_found("Version not found"))?;

    // Record the download (engine-portable upsert via the DAL).
    let _ = dal::downloads::record(&state.db, &name, &version).await;

    // GitHub-linked packages: redirect to upstream tarball
    if let Some(url) = target.tarball_url {
        if !url.is_empty() {
            return Ok(Redirect::temporary(&url).into_response());
        }
    }

    // Upload-sourced packages: serve blob from disk
    let data = blob::read(&state.config.blob_dir, &target.blob_key)
        .await
        .ok_or_else(|| ApiError::internal("Blob not found on disk"))?;

    let filename = format!("{}-{}.tar.gz", name.replace('/', "-"), version);
    Ok((
        [
            (header::CONTENT_TYPE, "application/gzip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        Body::from(data),
    )
        .into_response())
}

// ── Download Stats ──

pub async fn download_stats(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let total = dal::downloads::total(&state.db, &name).await.unwrap_or(0);

    // Daily counts (last 90 days). The cutoff date is computed in Rust and
    // bound, so no engine-specific date function is needed.
    let cutoff = crate::dal::time::date_days_ago(90);
    let daily: Vec<serde_json::Value> = dal::downloads::daily_since(&state.db, &name, &cutoff)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(date, count)| serde_json::json!({ "date": date, "count": count }))
        .collect();

    let versions: serde_json::Map<String, serde_json::Value> =
        dal::downloads::per_version(&state.db, &name)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(version, total)| (version, serde_json::json!(total)))
            .collect();

    Json(serde_json::json!({
        "package": name,
        "total": total,
        "daily": daily,
        "versions": versions,
    }))
}

// ── Search ──

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchQuery>,
) -> impl IntoResponse {
    let q = params.q.unwrap_or_default();
    let per_page = params.per_page.unwrap_or(20).min(100);
    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * per_page;

    let packages: Vec<serde_json::Value> = dal::packages::search(&state.db, &q, per_page, offset)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(name, description, created_at)| {
            serde_json::json!({
                "name": name,
                "description": description,
                "created_at": created_at,
            })
        })
        .collect();

    let total = dal::packages::search_count(&state.db, &q)
        .await
        .unwrap_or(0);

    Json(serde_json::json!({
        "packages": packages,
        "total": total,
        "page": page,
        "per_page": per_page,
    }))
}

// ── Yank ──

pub async fn yank(
    State(state): State<Arc<AppState>>,
    TokenUser { user, .. }: TokenUser,
    Path((name, version)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    // Resolve the package id only if the caller owns it.
    let package_id = dal::owners::package_id_if_owner(&state.db, &name, user.id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::forbidden("Not an owner"))?;

    let rows_affected = dal::versions::yank(&state.db, package_id, &version)
        .await
        .unwrap_or(0);

    if rows_affected > 0 {
        crate::audit::log(
            &state.db,
            &user.username,
            "yank",
            Some("version"),
            Some(&name),
            Some(&version),
        )
        .await;
        Ok(Json(serde_json::json!({"ok": true})))
    } else {
        Err(ApiError::not_found("Version not found"))
    }
}

// ── Ownership ──

pub async fn list_owners(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let owners = match dal::packages::find_by_name(&state.db, &name).await {
        Ok(Some(pkg)) => dal::owners::list_usernames(&state.db, pkg.id)
            .await
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    Json(serde_json::json!({ "owners": owners }))
}

#[derive(Deserialize)]
pub struct OwnerRequest {
    pub username: String,
}

pub async fn add_owner(
    State(state): State<Arc<AppState>>,
    TokenUser { user, .. }: TokenUser,
    Path(name): Path<String>,
    Json(body): Json<OwnerRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Check caller is an owner
    let pkg_id = dal::owners::package_id_if_owner(&state.db, &name, user.id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::forbidden("Not an owner or package not found"))?;

    // Find the target user
    let new_owner_id = dal::users::find_by_username(&state.db, &body.username)
        .await
        .ok()
        .flatten()
        .map(|u| u.id)
        .ok_or_else(|| ApiError::not_found("User not found"))?;

    // INSERT OR IGNORE
    let _ = dal::owners::add(&state.db, pkg_id, new_owner_id).await;

    crate::audit::log(
        &state.db,
        &user.username,
        "add_owner",
        Some("package"),
        Some(&name),
        Some(&body.username),
    )
    .await;

    Ok(Json(serde_json::json!({"ok": true})))
}

pub async fn remove_owner(
    State(state): State<Arc<AppState>>,
    TokenUser { user, .. }: TokenUser,
    Path(name): Path<String>,
    Json(body): Json<OwnerRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Check caller is an owner
    let pkg_id = dal::owners::package_id_if_owner(&state.db, &name, user.id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::forbidden("Not an owner or package not found"))?;

    // Check owner count
    let owner_count = dal::owners::count(&state.db, pkg_id).await.unwrap_or(0);

    if owner_count <= 1 {
        return Err(ApiError::bad_request("Cannot remove the last owner"));
    }

    // Find target user and delete ownership
    if let Some(target_user) = dal::users::find_by_username(&state.db, &body.username)
        .await
        .ok()
        .flatten()
    {
        let _ = dal::owners::remove(&state.db, pkg_id, target_user.id).await;
    }

    crate::audit::log(
        &state.db,
        &user.username,
        "remove_owner",
        Some("package"),
        Some(&name),
        Some(&body.username),
    )
    .await;

    Ok(Json(serde_json::json!({"ok": true})))
}
