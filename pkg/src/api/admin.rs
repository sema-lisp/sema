use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use super::ApiError;
use crate::auth::AuthUser;
use crate::{audit, auth::AdminUser, AppState};

// ── Dashboard ──

pub async fn stats(
    State(state): State<Arc<AppState>>,
    AdminUser(_user): AdminUser,
) -> impl IntoResponse {
    let s = crate::dal::admin::stats(&state.db).await;

    Json(serde_json::json!({
        "total_users": s.total_users,
        "total_packages": s.total_packages,
        "banned_users": s.banned_users,
        "open_reports": s.open_reports,
        "total_downloads": s.total_downloads,
    }))
}

// ── Users ──

#[derive(Deserialize)]
pub struct UserListParams {
    pub q: Option<String>,
    pub status: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

pub async fn list_users(
    State(state): State<Arc<AppState>>,
    AdminUser(_user): AdminUser,
    Query(params): Query<UserListParams>,
) -> impl IntoResponse {
    let per_page = params.per_page.unwrap_or(50).min(200);
    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * per_page;

    let filter = crate::dal::admin::UserListFilter {
        q: params.q,
        status: params.status,
        limit: per_page,
        offset,
    };
    let (rows, total) = crate::dal::admin::list_users(&state.db, &filter).await;

    let users: Vec<serde_json::Value> = rows
        .iter()
        .map(|u| {
            serde_json::json!({
                "id": u.id,
                "username": u.username,
                "email": u.email,
                "is_admin": u.is_admin,
                "github_id": u.github_id,
                "github_login": u.github_login,
                "package_count": u.package_count,
                "token_count": u.token_count,
                "banned": u.banned,
                "created_at": u.created_at,
            })
        })
        .collect();

    Json(serde_json::json!({
        "users": users,
        "total": total,
        "page": page,
        "per_page": per_page,
    }))
}

pub async fn get_user(
    State(state): State<Arc<AppState>>,
    AdminUser(_admin): AdminUser,
    Path(user_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let user_model = crate::dal::users::find_by_id(&state.db, user_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::not_found("User not found"))?;

    let package_names = crate::dal::owners::package_names_for_user(&state.db, user_id).await;

    let token_count = crate::dal::tokens::count_active_for_user(&state.db, user_id).await;

    let pkg_count = crate::dal::owners::count_for_user(&state.db, user_id).await;

    Ok(Json(serde_json::json!({
        "user": {
            "id": user_model.id,
            "username": user_model.username,
            "email": user_model.email,
            "is_admin": user_model.is_admin != 0,
            "github_id": user_model.github_id,
            "banned": user_model.banned_at.is_some(),
            "created_at": user_model.created_at,
        },
        "packages": package_names,
        "package_count": pkg_count,
        "active_token_count": token_count,
    })))
}

#[derive(Deserialize)]
pub struct BanRequest {
    pub reason: Option<String>,
}

pub async fn ban_user(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(user_id): Path<i64>,
    body: Option<Json<BanRequest>>,
) -> Result<impl IntoResponse, ApiError> {
    if user_id == admin.id {
        return Err(ApiError::bad_request("Cannot ban yourself"));
    }

    let reason = body.and_then(|b| b.0.reason);

    // Verify user exists
    let username = crate::dal::users::find_by_id(&state.db, user_id)
        .await
        .ok()
        .flatten()
        .map(|u| u.username)
        .ok_or_else(|| ApiError::not_found("User not found"))?;

    // Ban the user
    let _ = crate::dal::users::set_banned(&state.db, user_id, true).await;

    // Revoke all active tokens
    let _ = crate::dal::tokens::revoke_all_for_user(&state.db, user_id).await;

    // Delete all sessions
    let _ = crate::dal::sessions::delete_all_for_user(&state.db, user_id).await;

    let detail = reason.as_deref().unwrap_or("no reason given");
    audit::log(
        &state.db,
        &admin.username,
        "ban_user",
        Some("user"),
        Some(&username),
        Some(detail),
    )
    .await;

    Ok(Json(serde_json::json!({"ok": true})))
}

pub async fn unban_user(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(user_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let username = crate::dal::users::find_by_id(&state.db, user_id)
        .await
        .ok()
        .flatten()
        .map(|u| u.username)
        .ok_or_else(|| ApiError::not_found("User not found"))?;

    let _ = crate::dal::users::set_banned(&state.db, user_id, false).await;

    audit::log(
        &state.db,
        &admin.username,
        "unban_user",
        Some("user"),
        Some(&username),
        None,
    )
    .await;

    Ok(Json(serde_json::json!({"ok": true})))
}

pub async fn revoke_user_tokens(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(user_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let username = crate::dal::users::find_by_id(&state.db, user_id)
        .await
        .ok()
        .flatten()
        .map(|u| u.username)
        .ok_or_else(|| ApiError::not_found("User not found"))?;

    let count = crate::dal::tokens::revoke_all_for_user(&state.db, user_id).await;

    audit::log(
        &state.db,
        &admin.username,
        "revoke_tokens",
        Some("user"),
        Some(&username),
        Some(&format!("revoked {count} tokens")),
    )
    .await;

    Ok(Json(serde_json::json!({"ok": true, "revoked": count})))
}

#[derive(Deserialize)]
pub struct RoleRequest {
    pub is_admin: bool,
}

pub async fn set_user_role(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(user_id): Path<i64>,
    Json(body): Json<RoleRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if user_id == admin.id {
        return Err(ApiError::bad_request("Cannot change your own admin role"));
    }

    let username = crate::dal::users::find_by_id(&state.db, user_id)
        .await
        .ok()
        .flatten()
        .map(|u| u.username)
        .ok_or_else(|| ApiError::not_found("User not found"))?;

    let _ = crate::dal::users::set_admin(&state.db, user_id, body.is_admin).await;

    let role_str = if body.is_admin { "admin" } else { "user" };
    audit::log(
        &state.db,
        &admin.username,
        "set_role",
        Some("user"),
        Some(&username),
        Some(&format!("set role to {role_str}")),
    )
    .await;

    Ok(Json(serde_json::json!({"ok": true})))
}

// ── Packages ──

#[derive(Deserialize)]
pub struct PkgListParams {
    pub q: Option<String>,
    pub source: Option<String>,
    pub reported: Option<bool>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

pub async fn list_packages(
    State(state): State<Arc<AppState>>,
    AdminUser(_user): AdminUser,
    Query(params): Query<PkgListParams>,
) -> impl IntoResponse {
    let per_page = params.per_page.unwrap_or(50).min(200);
    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * per_page;

    let filter = crate::dal::admin::PkgListFilter {
        q: params.q,
        source: params.source,
        reported: params.reported,
        limit: per_page,
        offset,
    };
    let (rows, total) = crate::dal::admin::list_packages(&state.db, &filter).await;

    let packages: Vec<serde_json::Value> = rows
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "description": p.description,
                "latest_version": p.latest_version,
                "version_count": p.version_count,
                "source": p.source,
                "owner": p.owner,
                "downloads": p.downloads,
                "reported": p.reported,
                "created_at": p.created_at,
            })
        })
        .collect();

    Json(serde_json::json!({
        "packages": packages,
        "total": total,
        "page": page,
        "per_page": per_page,
    }))
}

pub async fn get_package(
    State(state): State<Arc<AppState>>,
    AdminUser(_user): AdminUser,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let pkg = crate::dal::packages::find_by_name(&state.db, &name)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ApiError::not_found("Package not found"))?;

    let pkg_id = pkg.id;

    let versions = crate::dal::versions::list_for_package(&state.db, pkg_id)
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
                "published_at": v.published_at,
            })
        })
        .collect();

    let owners = crate::dal::owners::list_usernames(&state.db, pkg_id)
        .await
        .unwrap_or_default();

    let open_reports = crate::dal::reports::count_open(&state.db, "package", &name).await;

    let dl_count = crate::dal::downloads::total(&state.db, &name)
        .await
        .unwrap_or(0);

    Ok(Json(serde_json::json!({
        "package": {
            "name": pkg.name,
            "description": pkg.description,
            "repository_url": pkg.repository_url,
            "source": pkg.source,
            "github_repo": pkg.github_repo,
            "created_at": pkg.created_at,
        },
        "versions": version_list,
        "owners": owners,
        "open_reports": open_reports,
        "total_downloads": dl_count,
    })))
}

pub async fn yank_all_versions(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let count = crate::dal::packages::yank_all(&state.db, &name).await;

    if count == 0 {
        return Err(ApiError::not_found(
            "Package not found or no versions to yank",
        ));
    }

    audit::log(
        &state.db,
        &admin.username,
        "yank_all",
        Some("package"),
        Some(&name),
        Some(&format!("yanked {count} versions")),
    )
    .await;

    Ok(Json(serde_json::json!({"ok": true, "yanked": count})))
}

pub async fn remove_package(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    if !crate::dal::packages::delete_by_name(&state.db, &name).await {
        return Err(ApiError::not_found("Package not found"));
    }

    audit::log(
        &state.db,
        &admin.username,
        "remove_package",
        Some("package"),
        Some(&name),
        None,
    )
    .await;

    Ok(Json(serde_json::json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct TransferRequest {
    pub to_username: String,
}

pub async fn transfer_ownership(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(name): Path<String>,
    Json(body): Json<TransferRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let pkg_id = crate::dal::packages::find_by_name(&state.db, &name)
        .await
        .ok()
        .flatten()
        .map(|p| p.id)
        .ok_or_else(|| ApiError::not_found("Package not found"))?;

    let target_id = crate::dal::users::find_by_username(&state.db, &body.to_username)
        .await
        .ok()
        .flatten()
        .map(|u| u.id)
        .ok_or_else(|| ApiError::not_found("Target user not found"))?;

    let _ = crate::dal::owners::transfer(&state.db, pkg_id, target_id).await;

    audit::log(
        &state.db,
        &admin.username,
        "transfer_ownership",
        Some("package"),
        Some(&name),
        Some(&format!("transferred to {}", body.to_username)),
    )
    .await;

    Ok(Json(serde_json::json!({"ok": true})))
}

// ── Audit Log ──

#[derive(Deserialize)]
pub struct AuditListParams {
    pub q: Option<String>,
    pub action: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

pub async fn list_audit(
    State(state): State<Arc<AppState>>,
    AdminUser(_user): AdminUser,
    Query(params): Query<AuditListParams>,
) -> impl IntoResponse {
    let per_page = params.per_page.unwrap_or(50).min(200);
    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * per_page;

    let filter = crate::dal::audit_log::AuditFilter {
        q: params.q,
        action: params.action,
        limit: per_page,
        offset,
    };
    let (rows, total) = crate::dal::audit_log::list(&state.db, &filter).await;

    let entries: Vec<serde_json::Value> = rows
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "actor": e.actor,
                "action": e.action,
                "target_type": e.target_type,
                "target_name": e.target_name,
                "detail": e.detail,
                "created_at": e.created_at,
            })
        })
        .collect();

    Json(serde_json::json!({
        "entries": entries,
        "total": total,
        "page": page,
        "per_page": per_page,
    }))
}

// ── Reports ──

#[derive(Deserialize)]
pub struct ReportListParams {
    pub status: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

pub async fn list_reports(
    State(state): State<Arc<AppState>>,
    AdminUser(_user): AdminUser,
    Query(params): Query<ReportListParams>,
) -> impl IntoResponse {
    let per_page = params.per_page.unwrap_or(50).min(200);
    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * per_page;

    let status = params.status.unwrap_or_else(|| "open".to_string());

    let (rows, total) = crate::dal::reports::list(&state.db, &status, per_page, offset).await;

    let reports: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "reporter": r.reporter.clone().unwrap_or_else(|| "[deleted]".to_string()),
                "target_type": r.target_type,
                "target_name": r.target_name,
                "report_type": r.report_type,
                "reason": r.reason,
                "status": r.status,
                "created_at": r.created_at,
            })
        })
        .collect();

    Json(serde_json::json!({
        "reports": reports,
        "total": total,
        "page": page,
        "per_page": per_page,
    }))
}

pub async fn action_report(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(report_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let affected = crate::dal::reports::resolve(&state.db, report_id, admin.id, "actioned").await;

    if affected > 0 {
        audit::log(
            &state.db,
            &admin.username,
            "action_report",
            Some("report"),
            Some(&report_id.to_string()),
            None,
        )
        .await;

        Ok(Json(serde_json::json!({"ok": true})))
    } else {
        Err(ApiError::not_found("Report not found or already resolved"))
    }
}

pub async fn dismiss_report(
    State(state): State<Arc<AppState>>,
    AdminUser(admin): AdminUser,
    Path(report_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let affected = crate::dal::reports::resolve(&state.db, report_id, admin.id, "dismissed").await;

    if affected > 0 {
        audit::log(
            &state.db,
            &admin.username,
            "dismiss_report",
            Some("report"),
            Some(&report_id.to_string()),
            None,
        )
        .await;

        Ok(Json(serde_json::json!({"ok": true})))
    } else {
        Err(ApiError::not_found("Report not found or already resolved"))
    }
}

// ── Report Submission (non-admin) ──

#[derive(Deserialize)]
pub struct SubmitReportRequest {
    pub target_type: String,
    pub target_name: String,
    pub report_type: String,
    pub reason: String,
}

pub async fn submit_report(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Json(body): Json<SubmitReportRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Validate target_type
    if !matches!(body.target_type.as_str(), "package" | "user") {
        return Err(ApiError::bad_request(
            "target_type must be 'package' or 'user'",
        ));
    }

    // Validate report_type
    if !matches!(
        body.report_type.as_str(),
        "spam" | "malware" | "abuse" | "other"
    ) {
        return Err(ApiError::bad_request(
            "report_type must be 'spam', 'malware', 'abuse', or 'other'",
        ));
    }

    // Validate lengths
    if body.target_name.is_empty() || body.target_name.len() > 200 {
        return Err(ApiError::bad_request(
            "target_name must be 1-200 characters",
        ));
    }

    if body.reason.is_empty() || body.reason.len() > 2000 {
        return Err(ApiError::bad_request("reason must be 1-2000 characters"));
    }

    crate::dal::reports::create(
        &state.db,
        user.id,
        body.target_type,
        body.target_name,
        body.report_type,
        body.reason,
    )
    .await
    .map_err(|_| ApiError::internal("Failed to submit report"))?;

    Ok((StatusCode::CREATED, Json(serde_json::json!({"ok": true}))))
}
