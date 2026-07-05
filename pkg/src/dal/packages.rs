//! Package aggregate: lookups, creation, description updates, and search.
//!
//! Search uses standard `LIKE` with bound patterns so it lowers identically on
//! every backend; `created_at` is application-generated via [`time`].

use sea_orm::{
    sea_query::Expr, ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait,
    QueryFilter, Set, Statement,
};

use crate::dal::time;
use crate::entity::package;

/// Look up a package by its unique name.
pub async fn find_by_name<C: ConnectionTrait>(
    db: &C,
    name: &str,
) -> Result<Option<package::Model>, DbErr> {
    package::Entity::find()
        .filter(package::Column::Name.eq(name))
        .one(db)
        .await
}

/// Insert a new upload-sourced package, stamping `created_at` in Rust.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    name: &str,
    description: &str,
    repository_url: Option<String>,
) -> Result<package::Model, DbErr> {
    let row = package::ActiveModel {
        name: Set(name.to_string()),
        description: Set(description.to_string()),
        repository_url: Set(repository_url),
        source: Set("upload".into()),
        created_at: Set(time::now()),
        ..Default::default()
    };
    row.insert(db).await
}

/// Overwrite a package's description.
pub async fn update_description<C: ConnectionTrait>(
    db: &C,
    package_id: i64,
    description: &str,
) -> Result<(), DbErr> {
    package::Entity::update_many()
        .col_expr(package::Column::Description, Expr::value(description))
        .filter(package::Column::Id.eq(package_id))
        .exec(db)
        .await
        .map(|_| ())
}

/// A single search hit: `(name, description, created_at)`.
pub type SearchHit = (String, String, String);

/// Search packages whose name or description matches `q` (case-sensitive
/// `LIKE`), ordered by name, paginated with `limit`/`offset`.
pub async fn search<C: ConnectionTrait>(
    db: &C,
    q: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<SearchHit>, DbErr> {
    let pattern = format!("%{q}%");
    let rows = db
        .query_all(Statement::from_sql_and_values(
            db.get_database_backend(),
            r#"SELECT name, description, created_at FROM packages
               WHERE name LIKE $1 OR description LIKE $2
               ORDER BY name
               LIMIT $3 OFFSET $4"#,
            [
                pattern.clone().into(),
                pattern.into(),
                limit.into(),
                offset.into(),
            ],
        ))
        .await?;

    Ok(rows
        .iter()
        .filter_map(|r| {
            let name: String = r.try_get("", "name").ok()?;
            let description: String = r.try_get("", "description").ok()?;
            let created_at: String = r.try_get("", "created_at").ok()?;
            Some((name, description, created_at))
        })
        .collect())
}

/// Count packages matching the same predicate as [`search`].
pub async fn search_count<C: ConnectionTrait>(db: &C, q: &str) -> Result<i64, DbErr> {
    let pattern = format!("%{q}%");
    let row = db
        .query_one(Statement::from_sql_and_values(
            db.get_database_backend(),
            "SELECT COUNT(*) as cnt FROM packages WHERE name LIKE $1 OR description LIKE $2",
            [pattern.clone().into(), pattern.into()],
        ))
        .await?;
    Ok(row.and_then(|r| r.try_get("", "cnt").ok()).unwrap_or(0))
}
