//! API token (personal access token) storage and lookups.

use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, Set,
};

use crate::dal::time;
use crate::entity::api_token;

/// Create an API token for a user, stamping `created_at` in Rust.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    user_id: i64,
    name: &str,
    token_hash: &str,
) -> Result<api_token::Model, DbErr> {
    let new_token = api_token::ActiveModel {
        user_id: Set(user_id),
        name: Set(name.to_string()),
        token_hash: Set(token_hash.to_string()),
        created_at: Set(time::now()),
        ..Default::default()
    };
    new_token.insert(db).await
}

/// List a user's active (non-revoked) tokens, newest first.
pub async fn list_active_for_user<C: ConnectionTrait>(
    db: &C,
    user_id: i64,
) -> Vec<api_token::Model> {
    api_token::Entity::find()
        .filter(api_token::Column::UserId.eq(user_id))
        .filter(api_token::Column::RevokedAt.is_null())
        .order_by_desc(api_token::Column::CreatedAt)
        .all(db)
        .await
        .unwrap_or_default()
}

/// Revoke a token owned by `user_id`, stamping `revoked_at` in Rust. Returns
/// the number of rows affected (0 if the token is missing, not owned, or
/// already revoked).
pub async fn revoke<C: ConnectionTrait>(db: &C, token_id: i64, user_id: i64) -> u64 {
    api_token::Entity::update_many()
        .col_expr(api_token::Column::RevokedAt, Expr::value(time::now()))
        .filter(api_token::Column::Id.eq(token_id))
        .filter(api_token::Column::UserId.eq(user_id))
        .filter(api_token::Column::RevokedAt.is_null())
        .exec(db)
        .await
        .map(|r| r.rows_affected)
        .unwrap_or(0)
}

/// Revoke every active token owned by `user_id`, stamping `revoked_at` in Rust.
/// Returns the number of tokens revoked (0 if the user had none active).
pub async fn revoke_all_for_user<C: ConnectionTrait>(db: &C, user_id: i64) -> u64 {
    api_token::Entity::update_many()
        .col_expr(api_token::Column::RevokedAt, Expr::value(time::now()))
        .filter(api_token::Column::UserId.eq(user_id))
        .filter(api_token::Column::RevokedAt.is_null())
        .exec(db)
        .await
        .map(|r| r.rows_affected)
        .unwrap_or(0)
}

/// Count a user's active (non-revoked) tokens.
pub async fn count_active_for_user<C: ConnectionTrait>(db: &C, user_id: i64) -> i64 {
    api_token::Entity::find()
        .filter(api_token::Column::UserId.eq(user_id))
        .filter(api_token::Column::RevokedAt.is_null())
        .count(db)
        .await
        .unwrap_or(0) as i64
}

/// Look up an active (non-revoked) token by its hash.
pub async fn find_active_by_hash<C: ConnectionTrait>(
    db: &C,
    token_hash: &str,
) -> Result<Option<api_token::Model>, DbErr> {
    api_token::Entity::find()
        .filter(api_token::Column::TokenHash.eq(token_hash))
        .filter(api_token::Column::RevokedAt.is_null())
        .one(db)
        .await
}

/// Update a token's `last_used_at` to the current time. Best-effort at the call
/// site, so the error is returned for the caller to discard.
pub async fn touch_last_used<C: ConnectionTrait>(
    db: &C,
    token: api_token::Model,
) -> Result<(), DbErr> {
    let mut active: api_token::ActiveModel = token.into();
    active.last_used_at = Set(Some(time::now()));
    active.update(db).await.map(|_| ())
}
