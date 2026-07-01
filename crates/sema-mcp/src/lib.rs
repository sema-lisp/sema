pub mod builtin_docs;
pub mod builtins;
pub mod client;
pub mod docs_search;
pub mod notebook;
pub mod protocol;
pub mod server;
pub mod tools;

pub use builtins::register_mcp_builtins;
pub use client::{McpClient, McpClientConfig};
pub use server::{run_mcp_server, run_mcp_server_on};
