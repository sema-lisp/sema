//! Package-version aggregate: listing, existence, creation, download
//! resolution, and yanking.
//!
//! `published_at` is application-generated via [`time`]; the download-target
//! join is standard SQL with bound params.

use sea_orm::{
    sea_query::Expr, ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Set, Statement,
};

use crate::dal::time;
use crate::entity::package_version;

/// Where a download should be served from: either an upstream redirect
/// (`tarball_url`) or an on-disk blob (`blob_key`).
pub struct DownloadTarget {
    pub blob_key: String,
    pub tarball_url: Option<String>,
}

/// All versions of a package, newest first.
pub async fn list_for_package<C: ConnectionTrait>(
    db: &C,
    package_id: i64,
) -> Result<Vec<package_version::Model>, DbErr> {
    package_version::Entity::find()
        .filter(package_version::Column::PackageId.eq(package_id))
        .order_by_desc(package_version::Column::PublishedAt)
        .all(db)
        .await
}

/// Whether `(package_id, version)` already exists.
pub async fn exists<C: ConnectionTrait>(
    db: &C,
    package_id: i64,
    version: &str,
) -> Result<bool, DbErr> {
    let count = package_version::Entity::find()
        .filter(package_version::Column::PackageId.eq(package_id))
        .filter(package_version::Column::Version.eq(version))
        .count(db)
        .await?;
    Ok(count > 0)
}

/// Insert a new version row, stamping `published_at` in Rust.
#[allow(clippy::too_many_arguments)]
pub async fn create<C: ConnectionTrait>(
    db: &C,
    package_id: i64,
    version: &str,
    checksum_sha256: &str,
    blob_key: String,
    size_bytes: i64,
    sema_version_req: Option<String>,
) -> Result<package_version::Model, DbErr> {
    let row = package_version::ActiveModel {
        package_id: Set(package_id),
        version: Set(version.to_string()),
        checksum_sha256: Set(checksum_sha256.to_string()),
        blob_key: Set(blob_key),
        size_bytes: Set(size_bytes),
        sema_version_req: Set(sema_version_req),
        published_at: Set(time::now()),
        ..Default::default()
    };
    row.insert(db).await
}

/// Resolve a downloadable, non-yanked version by package name + version.
pub async fn download_target<C: ConnectionTrait>(
    db: &C,
    name: &str,
    version: &str,
) -> Result<Option<DownloadTarget>, DbErr> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            db.get_database_backend(),
            r#"SELECT pv.blob_key, pv.tarball_url FROM package_versions pv
               JOIN packages p ON p.id = pv.package_id
               WHERE p.name = $1 AND pv.version = $2 AND pv.yanked = 0"#,
            [name.into(), version.into()],
        ))
        .await?;

    Ok(row.map(|r| DownloadTarget {
        blob_key: r.try_get("", "blob_key").unwrap_or_default(),
        tarball_url: r.try_get("", "tarball_url").ok(),
    }))
}

/// Mark `(package_id, version)` yanked; returns the number of rows affected.
pub async fn yank<C: ConnectionTrait>(
    db: &C,
    package_id: i64,
    version: &str,
) -> Result<u64, DbErr> {
    let result = package_version::Entity::update_many()
        .col_expr(package_version::Column::Yanked, Expr::value(1))
        .filter(package_version::Column::PackageId.eq(package_id))
        .filter(package_version::Column::Version.eq(version))
        .exec(db)
        .await?;
    Ok(result.rows_affected)
}
