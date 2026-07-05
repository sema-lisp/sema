//! Session row storage and the session→user join used to authenticate cookies.

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, Set,
};

use crate::dal::time;
use crate::entity::{session, user};

/// Insert a session row. The caller supplies the session id and computed
/// `expires_at`; `created_at` is stamped here from the canonical clock.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    session_id: &str,
    user_id: i64,
    expires_at: String,
) -> Result<(), DbErr> {
    let model = session::ActiveModel {
        id: Set(session_id.to_string()),
        user_id: Set(user_id),
        expires_at: Set(expires_at),
        created_at: Set(time::now()),
    };
    model.insert(db).await.map(|_| ())
}

/// Delete a session row by id. Best-effort at the call site (logout), so this
/// returns the error for the caller to discard.
pub async fn delete<C: ConnectionTrait>(db: &C, session_id: &str) -> Result<(), DbErr> {
    session::Entity::delete_by_id(session_id.to_string())
        .exec(db)
        .await
        .map(|_| ())
}

/// Delete every session belonging to `user_id` (e.g. when the account is
/// banned). Best-effort: the error is returned for the caller to discard.
pub async fn delete_all_for_user<C: ConnectionTrait>(db: &C, user_id: i64) -> Result<(), DbErr> {
    session::Entity::delete_many()
        .filter(session::Column::UserId.eq(user_id))
        .exec(db)
        .await
        .map(|_| ())
}

/// Fetch a session together with its related user in one query. Returns `None`
/// if the session is missing, has no related user, or the query fails.
pub async fn find_with_user<C: ConnectionTrait>(
    db: &C,
    session_id: &str,
) -> Option<(session::Model, user::Model)> {
    let (session_model, user_model) = session::Entity::find_by_id(session_id.to_string())
        .find_also_related(user::Entity)
        .one(db)
        .await
        .ok()??;
    Some((session_model, user_model?))
}
