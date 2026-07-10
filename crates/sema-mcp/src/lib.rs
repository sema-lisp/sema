pub mod builtin_docs;
pub mod builtins;
pub mod client;
pub mod client_auth;
pub mod docs_search;
pub mod notebook;
pub mod oauth;
pub mod protocol;
pub mod server;
pub mod tools;

pub use builtins::{
    browser_open_allowed, close_handle, connect_from_config, gated_browser_opener,
    register_mcp_builtins, ConnectFailure, ConnectOpts,
};
pub use client::{McpClient, McpClientConfig, McpHttpConfig};
pub use client_auth::{login_interactive, mcp_login, mcp_login_token, mcp_logout};
pub use server::{run_mcp_server, run_mcp_server_on};

/// A random 128-bit value as 32 lowercase hex characters (a UUID v4 with the
/// dashes stripped — `crates/sema-mcp/src/notebook.rs` mints notebook ids the
/// same way). Reuses this crate's existing `uuid`/OS-RNG dependency rather than
/// pulling in a new one; the value returned is not itself meaningful as a UUID,
/// just a convenient 32-hex-char random token shape. Used by the workflow
/// dashboard (`sema::workflow_view`) to mint its per-process session-hardening
/// token (docs/plans/2026-06-24-workflow-mcp-auth.md §8).
pub fn random_hex_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
