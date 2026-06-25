pub mod builtin_docs;
pub mod docs_search;
pub mod notebook;
pub mod protocol;
pub mod server;
pub mod tools;

pub use server::{run_mcp_server, run_mcp_server_on};
