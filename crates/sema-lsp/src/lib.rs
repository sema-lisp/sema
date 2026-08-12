//! Language Server Protocol implementation for Sema.
//!
//! This crate is structured around a single-threaded actor: the async
//! tower-lsp [`server::Backend`] forwards each request over an mpsc channel to a
//! dedicated thread that owns all non-`Send` state ([`state::BackendState`]) and
//! runs the parser/evaluator. The modules are:
//!
//! - [`state`] — `BackendState` and the cached parse/import data it holds.
//! - [`handlers`] — one submodule per LSP endpoint family, each adding
//!   `handle_*` methods to `BackendState`.
//! - [`server`] — the tower-lsp `Backend`, the actor loop, request dispatch, and
//!   the stdin/stdout transport (frame normalization + `run_server`).
//! - [`definitions`] — scanning a parsed AST for user definitions/imports
//!   (the `(module ...)`-aware family; see its module doc for why it's
//!   split out from [`helpers`]).
//! - [`helpers`], [`scope`], [`builtin_docs`] — shared parsing/analysis support.

pub mod builtin_docs;
pub(crate) mod definitions;
pub(crate) mod handlers;
pub(crate) mod helpers;
pub mod scope;
pub(crate) mod server;
pub(crate) mod state;

// ── Public API ───────────────────────────────────────────────────

// Server entry point used by the `sema lsp` subcommand.
pub use server::run_server;

// Custom `sema/evalResult` notification + its params, used by command execution.
pub use handlers::command::{EvalResultNotification, EvalResultParams};

// Re-export public helpers for external consumers.
pub use definitions::{
    document_symbols_from_ast, extract_params, extract_params_from_ast, import_path_at_cursor,
    import_path_from_ast, import_paths_from_ast, user_definitions, user_definitions_from_ast,
    user_definitions_with_spans,
};
pub use helpers::{
    analyze_document, compile_diagnostics, error_span, extract_prefix, extract_symbol_at,
    find_enclosing_call, parse_diagnostics, resolve_import_path, span_to_range, top_level_ranges,
    utf16_to_byte_offset,
};

#[cfg(test)]
mod tests;
