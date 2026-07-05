//! Package ownership: membership checks and mutations.
//!
//! Ownership joins are standard SQL with bound params; the add path uses
//! SeaORM's `on_conflict` so a repeated add is a portable no-op.

use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, PaginatorTrait, QueryFilter, Set, Statement,
};

use crate::entity::owner;

/// Whether `user_id` owns `package_id`.
pub async fn is_owner<C: ConnectionTrait>(
    db: &C,
    package_id: i64,
    user_id: i64,
) -> Result<bool, DbErr> {
    let count = owner::Entity::find()
        .filter(owner::Column::PackageId.eq(package_id))
        .filter(owner::Column::UserId.eq(user_id))
        .count(db)
        .await?;
    Ok(count > 0)
}

/// Resolve a package's id by name, but only if `user_id` is one of its owners.
pub async fn package_id_if_owner<C: ConnectionTrait>(
    db: &C,
    name: &str,
    user_id: i64,
) -> Result<Option<i64>, DbErr> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            db.get_database_backend(),
            r#"SELECT p.id FROM packages p
               JOIN owners o ON o.package_id = p.id
               WHERE p.name = $1 AND o.user_id = $2"#,
            [name.into(), user_id.into()],
        ))
        .await?;
    Ok(row.and_then(|r| r.try_get("", "id").ok()))
}

/// Usernames of every owner of `package_id`.
pub async fn list_usernames<C: ConnectionTrait>(
    db: &C,
    package_id: i64,
) -> Result<Vec<String>, DbErr> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            db.get_database_backend(),
            r#"SELECT u.username FROM users u
               JOIN owners o ON o.user_id = u.id
               WHERE o.package_id = $1"#,
            [package_id.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get("", "username").ok())
        .collect())
}

/// Names of every package owned by `user_id`, alphabetical.
pub async fn package_names_for_user<C: ConnectionTrait>(db: &C, user_id: i64) -> Vec<String> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            db.get_database_backend(),
            r#"SELECT p.name FROM packages p
               JOIN owners o ON o.package_id = p.id
               WHERE o.user_id = $1
               ORDER BY p.name"#,
            [user_id.into()],
        ))
        .await
        .unwrap_or_default();
    rows.iter()
        .map(|r| r.try_get::<String>("", "name").unwrap_or_default())
        .collect()
}

/// Number of packages owned by `user_id`.
pub async fn count_for_user<C: ConnectionTrait>(db: &C, user_id: i64) -> i64 {
    owner::Entity::find()
        .filter(owner::Column::UserId.eq(user_id))
        .count(db)
        .await
        .unwrap_or(0) as i64
}

/// Add an owner; a duplicate `(package_id, user_id)` is silently ignored.
pub async fn add<C: ConnectionTrait>(db: &C, package_id: i64, user_id: i64) -> Result<(), DbErr> {
    let row = owner::ActiveModel {
        package_id: Set(package_id),
        user_id: Set(user_id),
    };
    owner::Entity::insert(row)
        .on_conflict(
            OnConflict::columns([owner::Column::PackageId, owner::Column::UserId])
                .do_nothing()
                .to_owned(),
        )
        .do_nothing()
        .exec(db)
        .await
        .map(|_| ())
}

/// Remove an owner. A no-op if the pairing does not exist.
pub async fn remove<C: ConnectionTrait>(
    db: &C,
    package_id: i64,
    user_id: i64,
) -> Result<(), DbErr> {
    owner::Entity::delete_many()
        .filter(owner::Column::PackageId.eq(package_id))
        .filter(owner::Column::UserId.eq(user_id))
        .exec(db)
        .await
        .map(|_| ())
}

/// Admin: replace a package's ownership with a single owner — drops every
/// existing owner, then inserts `user_id`. The delete is best-effort (matching
/// the original handler); the insert's result is returned.
pub async fn transfer<C: ConnectionTrait>(
    db: &C,
    package_id: i64,
    user_id: i64,
) -> Result<(), DbErr> {
    let _ = owner::Entity::delete_many()
        .filter(owner::Column::PackageId.eq(package_id))
        .exec(db)
        .await;

    let new_owner = owner::ActiveModel {
        package_id: Set(package_id),
        user_id: Set(user_id),
    };
    owner::Entity::insert(new_owner).exec(db).await.map(|_| ())
}

/// Number of owners of `package_id`.
pub async fn count<C: ConnectionTrait>(db: &C, package_id: i64) -> Result<u64, DbErr> {
    owner::Entity::find()
        .filter(owner::Column::PackageId.eq(package_id))
        .count(db)
        .await
}
