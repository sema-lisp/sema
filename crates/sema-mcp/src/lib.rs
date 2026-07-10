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

pub use builtins::{connect_from_config, register_mcp_builtins, ConnectFailure, ConnectOpts};
pub use client::{McpClient, McpClientConfig, McpHttpConfig};
pub use client_auth::{mcp_login, mcp_logout};
pub use server::{run_mcp_server, run_mcp_server_on};
