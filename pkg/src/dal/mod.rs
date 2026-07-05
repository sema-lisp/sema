//! Data Access Layer.
//!
//! All database access flows through this module so the registry stays
//! engine-portable (SQLite / PostgreSQL / MySQL) and testable in one place.
//! Two rules keep it portable:
//!
//! 1. **Timestamps are application-generated strings.** Columns are TEXT and
//!    the app writes canonical `YYYY-MM-DD HH:MM:SS` (and `YYYY-MM-DD` dates)
//!    via [`time`], rather than relying on engine-specific `datetime('now')`
//!    or `CURRENT_TIMESTAMP` defaults. Date-window filters bind a cutoff
//!    computed in Rust.
//! 2. **Upserts use SeaORM's `on_conflict`,** which lowers to each backend's
//!    dialect; any remaining raw SQL uses only standard constructs (joins,
//!    `GROUP BY`, `COUNT`, `COALESCE`, `LIKE`) with bound parameters.
//!
//! DAL functions take `&impl ConnectionTrait` so they compose with both a
//! pooled connection and a transaction.

pub mod deps;
pub mod downloads;
pub mod oauth;
pub mod owners;
pub mod packages;
pub mod time;
pub mod users;
pub mod versions;
