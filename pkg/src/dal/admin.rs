//! Admin console read models: the dashboard summary and the dynamic-filter
//! user/package listings.
//!
//! The listings build their WHERE clause from static fragment literals and push
//! every user value as a bound parameter, then interpolate only the assembled
//! fragment string into the query. No user value is ever formatted in, so the
//! pattern stays injection-safe and lowers identically on every backend. The
//! 30-day download window binds a cutoff computed in Rust (see
//! [`time::date_days_ago`]).

use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Statement, Value,
};

use crate::dal::time;
use crate::entity::{package, report, user};

/// Dashboard summary counters.
pub struct Stats {
    pub total_users: i64,
    pub total_packages: i64,
    pub banned_users: i64,
    pub open_reports: i64,
    pub total_downloads: i64,
}

/// Compute the dashboard summary. `total_downloads` is the rolling 30-day sum.
pub async fn stats<C: ConnectionTrait>(db: &C) -> Stats {
    let total_users = user::Entity::find().count(db).await.unwrap_or(0) as i64;

    let total_packages = package::Entity::find().count(db).await.unwrap_or(0) as i64;

    let banned_users = user::Entity::find()
        .filter(user::Column::BannedAt.is_not_null())
        .count(db)
        .await
        .unwrap_or(0) as i64;

    let open_reports = report::Entity::find()
        .filter(report::Column::Status.eq("open"))
        .count(db)
        .await
        .unwrap_or(0) as i64;

    let total_downloads: i64 = {
        let cutoff = time::date_days_ago(30);
        let result = db
            .query_one(Statement::from_sql_and_values(
                db.get_database_backend(),
                r#"SELECT COALESCE(SUM(count), 0) as cnt FROM download_daily WHERE download_date >= $1"#,
                [cutoff.into()],
            ))
            .await;
        match result {
            Ok(Some(row)) => row.try_get_by_index::<i64>(0).unwrap_or(0),
            _ => 0,
        }
    };

    Stats {
        total_users,
        total_packages,
        banned_users,
        open_reports,
        total_downloads,
    }
}

/// Filters for [`list_users`]. `q` matches username/email via `LIKE`; `status`
/// is one of `"banned"`, `"active"`, `"github"` (anything else is ignored).
pub struct UserListFilter {
    pub q: Option<String>,
    pub status: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

/// One row of the admin user listing.
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub is_admin: bool,
    pub github_id: Option<i64>,
    pub github_login: Option<String>,
    pub package_count: i64,
    pub token_count: i64,
    pub banned: bool,
    pub created_at: String,
}

/// List users matching `filter`, newest first. Returns the page of rows plus
/// the total number of matching users.
pub async fn list_users<C: ConnectionTrait>(
    db: &C,
    filter: &UserListFilter,
) -> (Vec<UserRow>, i64) {
    let mut where_clauses: Vec<String> = vec!["1=1".to_string()];
    let mut binds: Vec<Value> = Vec::new();

    if let Some(ref q) = filter.q {
        let pattern = format!("%{q}%");
        where_clauses.push("(u.username LIKE ? OR u.email LIKE ?)".to_string());
        binds.push(pattern.clone().into());
        binds.push(pattern.into());
    }

    match filter.status.as_deref() {
        Some("banned") => where_clauses.push("u.banned_at IS NOT NULL".to_string()),
        Some("active") => where_clauses.push("u.banned_at IS NULL".to_string()),
        Some("github") => where_clauses.push("u.github_id IS NOT NULL".to_string()),
        _ => {}
    }

    let where_sql = where_clauses.join(" AND ");

    // Get total count
    let count_sql = format!("SELECT COUNT(*) as cnt FROM users u WHERE {where_sql}");
    let count_result = db
        .query_one(Statement::from_sql_and_values(
            db.get_database_backend(),
            &count_sql,
            binds.clone(),
        ))
        .await;
    let total: i64 = match count_result {
        Ok(Some(row)) => row.try_get_by_index::<i64>(0).unwrap_or(0),
        _ => 0,
    };

    let sql = format!(
        r#"SELECT u.id, u.username, u.email, u.is_admin, u.github_id,
              oc.provider_login,
              (SELECT COUNT(*) FROM owners WHERE owners.user_id = u.id) as package_count,
              (SELECT COUNT(*) FROM api_tokens WHERE api_tokens.user_id = u.id AND api_tokens.revoked_at IS NULL) as token_count,
              u.banned_at, u.created_at
           FROM users u
           LEFT JOIN oauth_connections oc ON oc.user_id = u.id AND oc.provider = 'github' AND oc.revoked_at IS NULL
           WHERE {where_sql}
           ORDER BY u.created_at DESC
           LIMIT ? OFFSET ?"#
    );

    let mut all_binds = binds;
    all_binds.push(filter.limit.into());
    all_binds.push(filter.offset.into());

    let rows = db
        .query_all(Statement::from_sql_and_values(
            db.get_database_backend(),
            &sql,
            all_binds,
        ))
        .await
        .unwrap_or_default();

    let users = rows
        .iter()
        .map(|r| {
            let banned_at: Option<String> = r.try_get_by("banned_at").unwrap_or(None);
            let github_id: Option<i64> = r.try_get_by("github_id").unwrap_or(None);
            UserRow {
                id: r.try_get_by::<i64, _>("id").unwrap_or(0),
                username: r.try_get_by::<String, _>("username").unwrap_or_default(),
                email: r.try_get_by::<String, _>("email").unwrap_or_default(),
                is_admin: r.try_get_by::<i32, _>("is_admin").unwrap_or(0) != 0,
                github_id,
                github_login: r
                    .try_get_by::<Option<String>, _>("provider_login")
                    .unwrap_or(None),
                package_count: r.try_get_by::<i64, _>("package_count").unwrap_or(0),
                token_count: r.try_get_by::<i64, _>("token_count").unwrap_or(0),
                banned: banned_at.is_some(),
                created_at: r.try_get_by::<String, _>("created_at").unwrap_or_default(),
            }
        })
        .collect();

    (users, total)
}

/// Filters for [`list_packages`]. `q` matches the package name via `LIKE`;
/// `source` is an exact match; `reported == Some(true)` limits to packages with
/// an open report.
pub struct PkgListFilter {
    pub q: Option<String>,
    pub source: Option<String>,
    pub reported: Option<bool>,
    pub limit: i64,
    pub offset: i64,
}

/// One row of the admin package listing.
pub struct PkgRow {
    pub name: String,
    pub description: String,
    pub latest_version: Option<String>,
    pub version_count: i64,
    pub source: String,
    pub owner: Option<String>,
    pub downloads: i64,
    pub reported: bool,
    pub created_at: String,
}

/// List packages matching `filter`, newest first. Returns the page of rows plus
/// the total number of matching packages.
pub async fn list_packages<C: ConnectionTrait>(
    db: &C,
    filter: &PkgListFilter,
) -> (Vec<PkgRow>, i64) {
    let mut where_clauses: Vec<String> = vec!["1=1".to_string()];
    let mut binds: Vec<Value> = Vec::new();

    if let Some(ref q) = filter.q {
        let pattern = format!("%{q}%");
        where_clauses.push("p.name LIKE ?".to_string());
        binds.push(pattern.into());
    }

    if let Some(ref source) = filter.source {
        where_clauses.push("p.source = ?".to_string());
        binds.push(source.clone().into());
    }

    if filter.reported == Some(true) {
        where_clauses.push(
            "EXISTS (SELECT 1 FROM reports r WHERE r.target_type = 'package' AND r.target_name = p.name AND r.status = 'open')"
                .to_string(),
        );
    }

    let where_sql = where_clauses.join(" AND ");

    // Get total count
    let count_sql = format!("SELECT COUNT(*) as cnt FROM packages p WHERE {where_sql}");
    let count_result = db
        .query_one(Statement::from_sql_and_values(
            db.get_database_backend(),
            &count_sql,
            binds.clone(),
        ))
        .await;
    let total: i64 = match count_result {
        Ok(Some(row)) => row.try_get_by_index::<i64>(0).unwrap_or(0),
        _ => 0,
    };

    let sql = format!(
        r#"SELECT p.name, p.description, p.source, p.created_at,
              (SELECT pv.version FROM package_versions pv WHERE pv.package_id = p.id ORDER BY pv.published_at DESC LIMIT 1) as latest_version,
              (SELECT COUNT(*) FROM package_versions pv WHERE pv.package_id = p.id) as version_count,
              (SELECT u.username FROM users u JOIN owners o ON o.user_id = u.id WHERE o.package_id = p.id LIMIT 1) as owner,
              (SELECT COALESCE(SUM(count), 0) FROM download_daily dl WHERE dl.package_name = p.name) as downloads,
              EXISTS (SELECT 1 FROM reports r WHERE r.target_type = 'package' AND r.target_name = p.name AND r.status = 'open') as reported
           FROM packages p
           WHERE {where_sql}
           ORDER BY p.created_at DESC
           LIMIT ? OFFSET ?"#
    );

    let mut all_binds = binds;
    all_binds.push(filter.limit.into());
    all_binds.push(filter.offset.into());

    let rows = db
        .query_all(Statement::from_sql_and_values(
            db.get_database_backend(),
            &sql,
            all_binds,
        ))
        .await
        .unwrap_or_default();

    let packages = rows
        .iter()
        .map(|r| PkgRow {
            name: r.try_get_by::<String, _>("name").unwrap_or_default(),
            description: r.try_get_by::<String, _>("description").unwrap_or_default(),
            latest_version: r
                .try_get_by::<Option<String>, _>("latest_version")
                .unwrap_or(None),
            version_count: r.try_get_by::<i64, _>("version_count").unwrap_or(0),
            source: r.try_get_by::<String, _>("source").unwrap_or_default(),
            owner: r.try_get_by::<Option<String>, _>("owner").unwrap_or(None),
            downloads: r.try_get_by::<i64, _>("downloads").unwrap_or(0),
            reported: r.try_get_by::<i32, _>("reported").unwrap_or(0) != 0,
            created_at: r.try_get_by::<String, _>("created_at").unwrap_or_default(),
        })
        .collect();

    (packages, total)
}
