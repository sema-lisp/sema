use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect},
    Json,
};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::*;
use serde::Deserialize;
use std::sync::Arc;

use super::ApiError;
use crate::{
    auth::TokenUser,
    blob,
    entity::{dependency, owner, package, package_version, user},
    AppState,
};

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
    let existing = package::Entity::find()
        .filter(package::Column::Name.eq(&name))
        .one(&state.db)
        .await
        .map_err(|_| ApiError::internal("Database error"))?;

    if let Some(pkg) = &existing {
        if pkg.source == "github" {
            return Err(ApiError::forbidden(
                "This package is GitHub-linked and cannot be published via CLI. Push a new semver tag to the linked repository instead.",
            ));
        }

        let is_owner = owner::Entity::find()
            .filter(owner::Column::PackageId.eq(pkg.id))
            .filter(owner::Column::UserId.eq(user.id))
            .count(&state.db)
            .await
            .unwrap_or(0);

        if is_owner == 0 {
            return Err(ApiError::forbidden("You are not an owner of this package"));
        }
    }

    let version_str = ver.to_string();
    if let Some(pkg) = &existing {
        let exists = package_version::Entity::find()
            .filter(package_version::Column::PackageId.eq(pkg.id))
            .filter(package_version::Column::Version.eq(&version_str))
            .count(&state.db)
            .await
            .unwrap_or(0);

        if exists > 0 {
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
            let new_pkg = package::ActiveModel {
                name: Set(name.clone()),
                description: Set(metadata.description.clone()),
                repository_url: Set(metadata.repository_url.clone()),
                source: Set("upload".into()),
                ..Default::default()
            };
            let pkg = new_pkg
                .insert(&txn)
                .await
                .map_err(|_| ApiError::internal("Failed to create package"))?;

            let new_owner = owner::ActiveModel {
                package_id: Set(pkg.id),
                user_id: Set(user.id),
            };
            new_owner
                .insert(&txn)
                .await
                .map_err(|_| ApiError::internal("Failed to create package owner"))?;

            pkg.id
        }
    };

    let new_version = package_version::ActiveModel {
        package_id: Set(package_id),
        version: Set(version_str.clone()),
        checksum_sha256: Set(checksum.clone()),
        blob_key: Set(blob_key),
        size_bytes: Set(size as i64),
        sema_version_req: Set(metadata.sema_version_req.clone()),
        ..Default::default()
    };

    let version_model = new_version
        .insert(&txn)
        .await
        .map_err(|_| ApiError::internal("Failed to insert version"))?;

    for dep in &metadata.dependencies {
        let new_dep = dependency::ActiveModel {
            version_id: Set(version_model.id),
            dependency_name: Set(dep.name.clone()),
            version_req: Set(dep.version_req.clone()),
            ..Default::default()
        };
        new_dep
            .insert(&txn)
            .await
            .map_err(|_| ApiError::internal("Failed to insert dependency"))?;
    }

    // Refresh the description on existing packages (new ones got it at insert)
    if existing.is_some() && !metadata.description.is_empty() {
        package::Entity::update_many()
            .col_expr(
                package::Column::Description,
                Expr::value(&metadata.description),
            )
            .filter(package::Column::Id.eq(package_id))
            .exec(&txn)
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
    let pkg = package::Entity::find()
        .filter(package::Column::Name.eq(&name))
        .one(&state.db)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::not_found("Package not found"))?;

    let pkg_id = pkg.id;

    let versions = package_version::Entity::find()
        .filter(package_version::Column::PackageId.eq(pkg_id))
        .order_by_desc(package_version::Column::PublishedAt)
        .all(&state.db)
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

    // Owners via join query
    let owner_rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            state.db.get_database_backend(),
            "SELECT u.username FROM users u JOIN owners o ON o.user_id = u.id WHERE o.package_id = $1",
            [pkg_id.into()],
        ))
        .await
        .unwrap_or_default();

    let owners: Vec<String> = owner_rows
        .iter()
        .filter_map(|r| r.try_get("", "username").ok())
        .collect();

    // Total downloads
    let dl_row = state
        .db
        .query_one(Statement::from_sql_and_values(
            state.db.get_database_backend(),
            "SELECT COALESCE(SUM(count), 0) as cnt FROM download_daily WHERE package_name = $1",
            [name.clone().into()],
        ))
        .await
        .ok()
        .flatten();

    let dl_count: i64 = dl_row.and_then(|r| r.try_get("", "cnt").ok()).unwrap_or(0);

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
    // Join query to find the version row
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            state.db.get_database_backend(),
            r#"SELECT pv.blob_key, pv.tarball_url FROM package_versions pv
               JOIN packages p ON p.id = pv.package_id
               WHERE p.name = $1 AND pv.version = $2 AND pv.yanked = 0"#,
            [name.clone().into(), version.clone().into()],
        ))
        .await
        .ok()
        .flatten();

    let row = row.ok_or_else(|| ApiError::not_found("Version not found"))?;

    // Record download (UPSERT) — raw SQL needed for date('now') expression
    let _ = state
        .db
        .execute(Statement::from_sql_and_values(
            state.db.get_database_backend(),
            "INSERT INTO download_daily (package_name, version, download_date, count) VALUES ($1, $2, date('now'), 1) ON CONFLICT(package_name, version, download_date) DO UPDATE SET count = count + 1",
            [name.clone().into(), version.clone().into()],
        ))
        .await;

    // GitHub-linked packages: redirect to upstream tarball
    let tarball_url: Option<String> = row.try_get("", "tarball_url").ok();
    if let Some(url) = tarball_url {
        if !url.is_empty() {
            return Ok(Redirect::temporary(&url).into_response());
        }
    }

    // Upload-sourced packages: serve blob from disk
    let blob_key: String = row.try_get("", "blob_key").unwrap_or_default();
    let data = blob::read(&state.config.blob_dir, &blob_key)
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
    let backend = state.db.get_database_backend();

    // Total downloads
    let total_row = state
        .db
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT COALESCE(SUM(count), 0) as cnt FROM download_daily WHERE package_name = $1",
            [name.clone().into()],
        ))
        .await
        .ok()
        .flatten();

    let total: i64 = total_row
        .and_then(|r| r.try_get("", "cnt").ok())
        .unwrap_or(0);

    // Daily counts (last 90 days)
    let daily_rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            backend,
            "SELECT download_date, SUM(count) as count FROM download_daily WHERE package_name = $1 AND download_date >= date('now', '-90 days') GROUP BY download_date ORDER BY download_date ASC",
            [name.clone().into()],
        ))
        .await
        .unwrap_or_default();

    let daily: Vec<serde_json::Value> = daily_rows
        .iter()
        .filter_map(|r| {
            let date: String = r.try_get("", "download_date").ok()?;
            let count: i64 = r.try_get("", "count").ok()?;
            Some(serde_json::json!({ "date": date, "count": count }))
        })
        .collect();

    // Per-version totals
    let version_rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            backend,
            "SELECT version, SUM(count) as total FROM download_daily WHERE package_name = $1 GROUP BY version ORDER BY total DESC",
            [name.clone().into()],
        ))
        .await
        .unwrap_or_default();

    let versions: serde_json::Map<String, serde_json::Value> = version_rows
        .iter()
        .filter_map(|r| {
            let version: String = r.try_get("", "version").ok()?;
            let total: i64 = r.try_get("", "total").ok()?;
            Some((version, serde_json::json!(total)))
        })
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
    let pattern = format!("%{q}%");

    let backend = state.db.get_database_backend();

    // Search with LIKE on name and description
    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            backend,
            r#"SELECT name, description, created_at FROM packages
               WHERE name LIKE $1 OR description LIKE $2
               ORDER BY name
               LIMIT $3 OFFSET $4"#,
            [
                pattern.clone().into(),
                pattern.clone().into(),
                per_page.into(),
                ((page - 1) * per_page).into(),
            ],
        ))
        .await
        .unwrap_or_default();

    let packages: Vec<serde_json::Value> = rows
        .iter()
        .filter_map(|r| {
            let name: String = r.try_get("", "name").ok()?;
            let description: String = r.try_get("", "description").ok()?;
            let created_at: String = r.try_get("", "created_at").ok()?;
            Some(serde_json::json!({
                "name": name,
                "description": description,
                "created_at": created_at,
            }))
        })
        .collect();

    let total_row = state
        .db
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT COUNT(*) as cnt FROM packages WHERE name LIKE $1 OR description LIKE $2",
            [pattern.clone().into(), pattern.into()],
        ))
        .await
        .ok()
        .flatten();

    let total: i64 = total_row
        .and_then(|r| r.try_get("", "cnt").ok())
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
    let backend = state.db.get_database_backend();

    // Check ownership via join
    let owner_row = state
        .db
        .query_one(Statement::from_sql_and_values(
            backend,
            r#"SELECT COUNT(*) as cnt FROM owners o
               JOIN packages p ON p.id = o.package_id
               WHERE p.name = $1 AND o.user_id = $2"#,
            [name.clone().into(), user.id.into()],
        ))
        .await
        .ok()
        .flatten();

    let is_owner: i64 = owner_row
        .and_then(|r| r.try_get("", "cnt").ok())
        .unwrap_or(0);

    if is_owner == 0 {
        return Err(ApiError::forbidden("Not an owner"));
    }

    // Find the package to get its ID for the update
    let pkg = package::Entity::find()
        .filter(package::Column::Name.eq(&name))
        .one(&state.db)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::not_found("Version not found"))?;

    let result = package_version::Entity::update_many()
        .col_expr(package_version::Column::Yanked, Expr::value(1))
        .filter(package_version::Column::PackageId.eq(pkg.id))
        .filter(package_version::Column::Version.eq(&version))
        .exec(&state.db)
        .await;

    match result {
        Ok(r) if r.rows_affected > 0 => {
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
        }
        _ => Err(ApiError::not_found("Version not found")),
    }
}

// ── Ownership ──

pub async fn list_owners(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            state.db.get_database_backend(),
            r#"SELECT u.username FROM users u
               JOIN owners o ON o.user_id = u.id
               JOIN packages p ON p.id = o.package_id
               WHERE p.name = $1"#,
            [name.into()],
        ))
        .await
        .unwrap_or_default();

    let owners: Vec<String> = rows
        .iter()
        .filter_map(|r| r.try_get("", "username").ok())
        .collect();

    Json(serde_json::json!({"owners": owners}))
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
    let backend = state.db.get_database_backend();

    // Check caller is an owner
    let pkg_row = state
        .db
        .query_one(Statement::from_sql_and_values(
            backend,
            r#"SELECT p.id FROM packages p
               JOIN owners o ON o.package_id = p.id
               WHERE p.name = $1 AND o.user_id = $2"#,
            [name.clone().into(), user.id.into()],
        ))
        .await
        .ok()
        .flatten();

    let pkg_id: i64 = pkg_row
        .and_then(|r| r.try_get("", "id").ok())
        .ok_or_else(|| ApiError::forbidden("Not an owner or package not found"))?;

    // Find the target user
    let new_owner_id = user::Entity::find()
        .filter(user::Column::Username.eq(&body.username))
        .one(&state.db)
        .await
        .ok()
        .flatten()
        .map(|u| u.id)
        .ok_or_else(|| ApiError::not_found("User not found"))?;

    // INSERT OR IGNORE
    let new_owner_model = owner::ActiveModel {
        package_id: Set(pkg_id),
        user_id: Set(new_owner_id),
    };
    let _ = owner::Entity::insert(new_owner_model)
        .on_conflict(
            OnConflict::columns([owner::Column::PackageId, owner::Column::UserId])
                .do_nothing()
                .to_owned(),
        )
        .do_nothing()
        .exec(&state.db)
        .await;

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
    let backend = state.db.get_database_backend();

    // Check caller is an owner
    let pkg_row = state
        .db
        .query_one(Statement::from_sql_and_values(
            backend,
            r#"SELECT p.id FROM packages p
               JOIN owners o ON o.package_id = p.id
               WHERE p.name = $1 AND o.user_id = $2"#,
            [name.clone().into(), user.id.into()],
        ))
        .await
        .ok()
        .flatten();

    let pkg_id: i64 = pkg_row
        .and_then(|r| r.try_get("", "id").ok())
        .ok_or_else(|| ApiError::forbidden("Not an owner or package not found"))?;

    // Check owner count
    let owner_count = owner::Entity::find()
        .filter(owner::Column::PackageId.eq(pkg_id))
        .count(&state.db)
        .await
        .unwrap_or(0);

    if owner_count <= 1 {
        return Err(ApiError::bad_request("Cannot remove the last owner"));
    }

    // Find target user and delete ownership
    let target = user::Entity::find()
        .filter(user::Column::Username.eq(&body.username))
        .one(&state.db)
        .await
        .ok()
        .flatten();

    if let Some(target_user) = target {
        let _ = owner::Entity::delete_many()
            .filter(owner::Column::PackageId.eq(pkg_id))
            .filter(owner::Column::UserId.eq(target_user.id))
            .exec(&state.db)
            .await;
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
