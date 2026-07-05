//! Audit-log read side. Writes live in [`crate::audit`]; this module only reads
//! back the trail for the admin console.
//!
//! The listing builds its WHERE clause from static fragment literals, pushing
//! every user value as a bound parameter, then interpolates only the assembled
//! fragment string into the query — injection-safe and portable across engines.

use sea_orm::{ConnectionTrait, Statement, Value};

/// Filters for [`list`]. `q` matches actor/target/detail via `LIKE`; `action`
/// is an exact match. `limit`/`offset` paginate.
pub struct AuditFilter {
    pub q: Option<String>,
    pub action: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

/// One audit-trail entry.
pub struct AuditRow {
    pub id: i64,
    pub actor: String,
    pub action: String,
    pub target_type: Option<String>,
    pub target_name: Option<String>,
    pub detail: Option<String>,
    pub created_at: String,
}

/// List audit entries matching `filter`, newest first. Returns the page of rows
/// plus the total number of matching entries.
pub async fn list<C: ConnectionTrait>(db: &C, filter: &AuditFilter) -> (Vec<AuditRow>, i64) {
    let mut where_clauses: Vec<String> = vec!["1=1".to_string()];
    let mut binds: Vec<Value> = Vec::new();

    if let Some(ref action) = filter.action {
        where_clauses.push("action = ?".to_string());
        binds.push(action.clone().into());
    }

    if let Some(ref q) = filter.q {
        let pattern = format!("%{q}%");
        where_clauses.push("(actor LIKE ? OR target_name LIKE ? OR detail LIKE ?)".to_string());
        binds.push(pattern.clone().into());
        binds.push(pattern.clone().into());
        binds.push(pattern.into());
    }

    let where_sql = where_clauses.join(" AND ");

    // Get total count
    let count_sql = format!("SELECT COUNT(*) as cnt FROM audit_log WHERE {where_sql}");
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

    // Get entries
    let sql = format!(
        r#"SELECT id, actor, action, target_type, target_name, detail, created_at
           FROM audit_log
           WHERE {where_sql}
           ORDER BY created_at DESC
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

    let entries = rows
        .iter()
        .map(|r| AuditRow {
            id: r.try_get_by::<i64, _>("id").unwrap_or(0),
            actor: r.try_get_by::<String, _>("actor").unwrap_or_default(),
            action: r.try_get_by::<String, _>("action").unwrap_or_default(),
            target_type: r
                .try_get_by::<Option<String>, _>("target_type")
                .unwrap_or(None),
            target_name: r
                .try_get_by::<Option<String>, _>("target_name")
                .unwrap_or(None),
            detail: r.try_get_by::<Option<String>, _>("detail").unwrap_or(None),
            created_at: r.try_get_by::<String, _>("created_at").unwrap_or_default(),
        })
        .collect();

    (entries, total)
}
