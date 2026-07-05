//! Daily download counters.
//!
//! The record path is an upsert expressed with SeaORM's `on_conflict` so it
//! lowers to each backend's dialect (`ON CONFLICT` on SQLite/Postgres,
//! `ON DUPLICATE KEY UPDATE` on MySQL). The date is generated in Rust.

use sea_orm::sea_query::OnConflict;
use sea_orm::{ConnectionTrait, DbErr, EntityTrait, Set, Statement};

use crate::dal::time;
use crate::entity::download_daily;

/// Increment today's download counter for `(package_name, version)`,
/// inserting the row on first download.
pub async fn record<C: ConnectionTrait>(
    db: &C,
    package_name: &str,
    version: &str,
) -> Result<(), DbErr> {
    let row = download_daily::ActiveModel {
        package_name: Set(package_name.to_string()),
        version: Set(version.to_string()),
        download_date: Set(time::today()),
        count: Set(1),
    };

    download_daily::Entity::insert(row)
        .on_conflict(
            OnConflict::columns([
                download_daily::Column::PackageName,
                download_daily::Column::Version,
                download_daily::Column::DownloadDate,
            ])
            .value(
                download_daily::Column::Count,
                sea_orm::sea_query::Expr::col(download_daily::Column::Count).add(1),
            )
            .to_owned(),
        )
        .exec(db)
        .await
        .map(|_| ())
}

/// All-time download total for a package (0 if never downloaded).
pub async fn total<C: ConnectionTrait>(db: &C, package_name: &str) -> Result<i64, DbErr> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            db.get_database_backend(),
            "SELECT COALESCE(SUM(count), 0) as cnt FROM download_daily WHERE package_name = $1",
            [package_name.into()],
        ))
        .await?;
    Ok(row.and_then(|r| r.try_get("", "cnt").ok()).unwrap_or(0))
}

/// Daily download totals `(date, count)` on/after `cutoff`, oldest first. The
/// cutoff is a caller-computed `YYYY-MM-DD` string (see [`time::date_days_ago`]),
/// so no engine-specific date function is needed.
pub async fn daily_since<C: ConnectionTrait>(
    db: &C,
    package_name: &str,
    cutoff: &str,
) -> Result<Vec<(String, i64)>, DbErr> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            db.get_database_backend(),
            "SELECT download_date, SUM(count) as count FROM download_daily WHERE package_name = $1 AND download_date >= $2 GROUP BY download_date ORDER BY download_date ASC",
            [package_name.into(), cutoff.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let date: String = r.try_get("", "download_date").ok()?;
            let count: i64 = r.try_get("", "count").ok()?;
            Some((date, count))
        })
        .collect())
}

/// Download totals per version `(version, total)`, most-downloaded first.
pub async fn per_version<C: ConnectionTrait>(
    db: &C,
    package_name: &str,
) -> Result<Vec<(String, i64)>, DbErr> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            db.get_database_backend(),
            "SELECT version, SUM(count) as total FROM download_daily WHERE package_name = $1 GROUP BY version ORDER BY total DESC",
            [package_name.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let version: String = r.try_get("", "version").ok()?;
            let total: i64 = r.try_get("", "total").ok()?;
            Some((version, total))
        })
        .collect())
}
