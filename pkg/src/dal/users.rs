//! User lookups needed by other aggregates (kept intentionally minimal).

use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DbErr, EntityTrait, QueryFilter,
    Set, Value,
};

use crate::dal::time;
use crate::entity::user;

/// Look up a user by id.
pub async fn find_by_id<C: ConnectionTrait>(
    db: &C,
    user_id: i64,
) -> Result<Option<user::Model>, DbErr> {
    user::Entity::find_by_id(user_id).one(db).await
}

/// Set or clear a user's banned state. Banning stamps `banned_at` with the
/// current time; unbanning clears it to NULL.
pub async fn set_banned<C: ConnectionTrait>(
    db: &C,
    user_id: i64,
    banned: bool,
) -> Result<(), DbErr> {
    let value = if banned {
        Expr::value(time::now())
    } else {
        Expr::value(Value::String(None))
    };
    user::Entity::update_many()
        .col_expr(user::Column::BannedAt, value)
        .filter(user::Column::Id.eq(user_id))
        .exec(db)
        .await
        .map(|_| ())
}

/// Grant or revoke a user's admin role.
pub async fn set_admin<C: ConnectionTrait>(
    db: &C,
    user_id: i64,
    is_admin: bool,
) -> Result<(), DbErr> {
    let admin_val: i32 = if is_admin { 1 } else { 0 };
    user::Entity::update_many()
        .col_expr(user::Column::IsAdmin, Expr::value(admin_val))
        .filter(user::Column::Id.eq(user_id))
        .exec(db)
        .await
        .map(|_| ())
}

/// Look up a user by their unique username.
pub async fn find_by_username<C: ConnectionTrait>(
    db: &C,
    username: &str,
) -> Result<Option<user::Model>, DbErr> {
    user::Entity::find()
        .filter(user::Column::Username.eq(username))
        .one(db)
        .await
}

/// Create a new user with a password hash, stamping `created_at` in Rust.
/// Uniqueness of `username`/`email` is enforced by the table's unique indexes,
/// so a duplicate surfaces as a `DbErr` from the insert.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    username: &str,
    email: &str,
    password_hash: &str,
) -> Result<user::Model, DbErr> {
    let new_user = user::ActiveModel {
        username: Set(username.to_string()),
        email: Set(email.to_string()),
        password_hash: Set(Some(password_hash.to_string())),
        created_at: Set(time::now()),
        ..Default::default()
    };
    new_user.insert(db).await
}

/// Look up a user by either their username or email (used at login, where the
/// same input field accepts both).
pub async fn find_by_username_or_email<C: ConnectionTrait>(
    db: &C,
    login: &str,
) -> Result<Option<user::Model>, DbErr> {
    user::Entity::find()
        .filter(
            Condition::any()
                .add(user::Column::Username.eq(login))
                .add(user::Column::Email.eq(login)),
        )
        .one(db)
        .await
}

/// Look up a user by id, excluding banned accounts (`banned_at` set).
pub async fn find_active_by_id<C: ConnectionTrait>(
    db: &C,
    user_id: i64,
) -> Result<Option<user::Model>, DbErr> {
    user::Entity::find_by_id(user_id)
        .filter(user::Column::BannedAt.is_null())
        .one(db)
        .await
}
