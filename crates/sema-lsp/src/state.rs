//! Backend state shared across all LSP request handlers.
//!
//! Holds the language server's in-memory view of the workspace: open documents,
//! cached parses (AST + spans + scope tree), the import cache for files not
//! currently open, and harvested builtin names/docs. Request handlers live in
//! the [`crate::handlers`] submodules and are implemented as `impl BackendState`
//! blocks there; this module owns the data and the cross-cutting helpers
//! (construction, the import cache, the shared definition index, and the
//! geometry helpers used by structural requests).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::*;

use sema_core::{Caps, Sandbox, Span, SpanMap};

use crate::builtin_docs;
use crate::helpers::*;
use crate::scope;

// ── Incremental workspace scanner ────────────────────────────────

/// Incremental workspace scanner state.
/// Walks directories one at a time, collecting `.sema` files and parsing them,
/// so the backend can yield to interactive requests between directories.
pub(crate) struct WorkspaceScanner {
    /// Directories remaining to visit.
    pub(crate) dir_stack: Vec<PathBuf>,
    /// Canonical paths already visited (symlink cycle protection).
    visited: std::collections::HashSet<PathBuf>,
    /// Files from the current directory not yet parsed (for batching large dirs).
    pub(crate) pending_files: Vec<PathBuf>,
}

impl WorkspaceScanner {
    pub(crate) fn new(root: &Path) -> Self {
        let mut visited = std::collections::HashSet::new();
        let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        visited.insert(canonical_root.clone());
        WorkspaceScanner {
            dir_stack: vec![canonical_root],
            visited,
            pending_files: Vec::new(),
        }
    }

    /// Process the next directory on the stack.
    /// Returns the `.sema` files found in that single directory.
    /// Returns `None` when the scan is complete (no more directories).
    pub(crate) fn next_dir(&mut self) -> Option<Vec<PathBuf>> {
        let dir = self.dir_stack.pop()?;
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Some(Vec::new()),
        };
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Skip hidden dirs, target, node_modules, .git
            if name_str.starts_with('.') || name_str == "target" || name_str == "node_modules" {
                continue;
            }
            // Follow symlinks for file discovery; cycles are detected via canonicalize + visited set
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let path = entry.path();
            if meta.is_dir() {
                if let Ok(canonical) = std::fs::canonicalize(&path) {
                    if self.visited.insert(canonical) {
                        self.dir_stack.push(path);
                    }
                }
            } else if meta.is_file() && path.extension().and_then(|e| e.to_str()) == Some("sema") {
                files.push(path);
            }
        }
        Some(files)
    }
}

// ── Cached parse results ─────────────────────────────────────────

/// Cached parse result for an imported file.
pub(crate) struct ImportCache {
    pub(crate) ast: Vec<sema_core::Value>,
    pub(crate) span_map: SpanMap,
    pub(crate) symbol_spans: Vec<(String, Span)>,
    pub(crate) scope_tree: scope::ScopeTree,
    /// Source text, retained so cross-file ranges can be mapped from char
    /// columns to UTF-16 code units (LSP `Position`). See `span_to_range`.
    pub(crate) source: String,
    /// Modification time when we last read the file.
    pub(crate) mtime: std::time::SystemTime,
}

/// Cached parse result for an open document (updated on every didChange).
pub(crate) struct CachedParse {
    pub(crate) ast: Vec<sema_core::Value>,
    pub(crate) span_map: SpanMap,
    pub(crate) symbol_spans: Vec<(String, Span)>,
    pub(crate) scope_tree: scope::ScopeTree,
    /// Source text of the document at parse time, retained so ranges can be
    /// mapped from char columns to UTF-16 code units (LSP `Position`).
    /// Mirrors `ImportCache::source`. See `span_to_range`.
    pub(crate) source: String,
}

// ── Semantic token legend ─────────────────────────────────────────

/// Indices into the token types legend for semantic tokens.
pub(crate) mod token_types {
    pub const KEYWORD: u32 = 0;
    pub const FUNCTION: u32 = 1;
    pub const VARIABLE: u32 = 2;
    pub const PARAMETER: u32 = 3;
    pub const MACRO: u32 = 4;
}

/// Indices into the token modifiers legend for semantic tokens.
pub(crate) mod token_modifiers {
    pub const DEFAULT_LIBRARY: u32 = 0b0000_0001;
}

pub(crate) fn semantic_token_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::KEYWORD,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PARAMETER,
            SemanticTokenType::MACRO,
        ],
        // Only DEFAULT_LIBRARY is emitted (semantic_tokens.rs); don't advertise a
        // modifier the server never sets.
        token_modifiers: vec![SemanticTokenModifier::DEFAULT_LIBRARY],
    }
}

// ── BackendState ──────────────────────────────────────────────────

pub(crate) struct BackendState {
    /// Cached builtin names (from stdlib env) — HashSet for O(1) lookups.
    pub(crate) builtin_names: HashSet<String>,
    /// Per-document source text.
    pub(crate) documents: HashMap<String, String>,
    /// Cached user definitions per document (from last successful parse).
    /// Avoids losing completions while the user is typing (syntax errors).
    pub(crate) cached_user_defs: HashMap<String, Vec<String>>,
    /// Structured builtin/special-form documentation (from the sema-docs index).
    pub(crate) builtin_docs: builtin_docs::BuiltinDocs,
    /// Cached parse results for imported files (by absolute path).
    pub(crate) import_cache: HashMap<PathBuf, ImportCache>,
    /// Cached parse results for open documents (updated on didChange).
    pub(crate) cached_parses: HashMap<String, CachedParse>,
    /// Path to the sema binary (from initializationOptions or default).
    pub(crate) sema_binary: String,
    /// Sandbox mode for code execution via Run code lens (e.g., "off", "strict").
    pub(crate) run_sandbox_mode: String,
}

/// Resolve the default `sema` binary used by the eval subprocess.
///
/// The language server runs *as* the `sema` binary (`sema lsp`), so
/// `std::env::current_exe()` is the most reliable self-reference: it points at the
/// exact binary the user launched, regardless of name or whether `sema` is on `PATH`.
/// Falls back to `"sema"` (a `PATH` lookup) if the current exe can't be determined.
/// The client may still override this via `initializationOptions.semaPath`.
pub(crate) fn default_sema_binary() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| "sema".to_string())
}

/// Whether `pos` falls within `range` (inclusive), comparing (line, character) lexicographically.
pub(crate) fn position_in_range(pos: &Position, range: &Range) -> bool {
    let p = (pos.line, pos.character);
    let start = (range.start.line, range.start.character);
    let end = (range.end.line, range.end.character);
    p >= start && p <= end
}

/// Build a nested [`SelectionRange`] (innermost first, parents pointing outward) from the set of
/// ranges that contain the cursor. Falls back to a zero-width range at `pos` when nothing matches.
pub(crate) fn build_selection_range(mut ranges: Vec<Range>, pos: &Position) -> SelectionRange {
    // Sort outermost → innermost: smaller start first, then larger end first.
    ranges.sort_by(|a, b| {
        (a.start.line, a.start.character)
            .cmp(&(b.start.line, b.start.character))
            .then((b.end.line, b.end.character).cmp(&(a.end.line, a.end.character)))
    });
    ranges.dedup();
    let mut node: Option<Box<SelectionRange>> = None;
    for range in ranges {
        node = Some(Box::new(SelectionRange {
            range,
            parent: node,
        }));
    }
    node.map(|b| *b).unwrap_or(SelectionRange {
        range: Range {
            start: *pos,
            end: *pos,
        },
        parent: None,
    })
}

/// Heads that introduce a top-level named definition.
pub(crate) const DEFINITION_HEADS: &[&str] =
    &["define", "defun", "defn", "defmacro", "defagent", "deftool"];

/// If `expr` is a definition form, return its `(name, full-form range, name range)`.
pub(crate) fn def_of_form(
    expr: &sema_core::Value,
    span_map: &SpanMap,
    symbol_spans: &[(String, Span)],
    lines: &[&str],
) -> Option<(String, Range, Range)> {
    let items = expr.as_list()?;
    if items.len() < 2 {
        return None;
    }
    let head = items[0].as_symbol()?;
    if !DEFINITION_HEADS.contains(&head.as_str()) {
        return None;
    }
    // `(define name ...)` or `(define (name args...) ...)` shorthand.
    let name = items[1].as_symbol().or_else(|| {
        items[1]
            .as_list()
            .and_then(|sig| sig.first().and_then(|v| v.as_symbol()))
    })?;
    let form_span = expr_span(expr, span_map)?;
    let form_range = span_to_range(form_span, lines);
    let name_range = find_name_span(&name, form_span, symbol_spans, lines).unwrap_or(form_range);
    Some((name, form_range, name_range))
}

/// Collect every call site of `target` (a list whose head symbol is `target`) within `exprs`,
/// recording the head symbol's range. Recurses into nested forms.
pub(crate) fn collect_call_sites(
    exprs: &[sema_core::Value],
    span_map: &SpanMap,
    symbol_spans: &[(String, Span)],
    lines: &[&str],
    target: &str,
    out: &mut Vec<Range>,
) {
    for expr in exprs {
        if let Some(items) = expr.as_list() {
            if items.first().and_then(|v| v.as_symbol()).as_deref() == Some(target) {
                if let Some(span) = expr_span(expr, span_map) {
                    let r = find_name_span(target, span, symbol_spans, lines)
                        .unwrap_or_else(|| span_to_range(span, lines));
                    out.push(r);
                }
            }
            collect_call_sites(items, span_map, symbol_spans, lines, target, out);
        }
    }
}

/// Walk `exprs`, recording call sites whose head symbol names a known definition (key in `index`),
/// grouped by callee name. Used for outgoing call hierarchy.
pub(crate) fn collect_outgoing_calls(
    exprs: &[sema_core::Value],
    span_map: &SpanMap,
    symbol_spans: &[(String, Span)],
    lines: &[&str],
    index: &std::collections::HashMap<String, (Url, Range, Range)>,
    out: &mut std::collections::HashMap<String, Vec<Range>>,
) {
    for expr in exprs {
        if let Some(items) = expr.as_list() {
            if let Some(head) = items.first().and_then(|v| v.as_symbol()) {
                if index.contains_key(&head) {
                    if let Some(span) = expr_span(expr, span_map) {
                        let r = find_name_span(&head, span, symbol_spans, lines)
                            .unwrap_or_else(|| span_to_range(span, lines));
                        out.entry(head).or_default().push(r);
                    }
                }
            }
            collect_outgoing_calls(items, span_map, symbol_spans, lines, index, out);
        }
    }
}

/// Range (UTF-16) of the quoted `path` literal on the form's start line, excluding the quotes.
/// Returns `None` for multi-line forms or when the literal can't be located verbatim.
pub(crate) fn quoted_string_range(lines: &[&str], form_range: &Range, path: &str) -> Option<Range> {
    let line_idx = form_range.start.line as usize;
    let line = lines.get(line_idx).copied()?;
    let needle = format!("\"{path}\"");
    let byte_pos = line.find(&needle)?;
    let quote_char = line[..byte_pos].chars().count();
    let inner_start = quote_char + 1; // first char inside the quotes
    let prefix_utf16: u32 = line
        .chars()
        .take(inner_start)
        .map(|c| c.len_utf16() as u32)
        .sum();
    let path_utf16: u32 = path.chars().map(|c| c.len_utf16() as u32).sum();
    Some(Range {
        start: Position {
            line: line_idx as u32,
            character: prefix_utf16,
        },
        end: Position {
            line: line_idx as u32,
            character: prefix_utf16 + path_utf16,
        },
    })
}

impl BackendState {
    pub(crate) fn new() -> Self {
        // Create a sandboxed interpreter just to harvest builtin names.
        let sandbox = Sandbox::deny(Caps::ALL);
        let interp = sema_eval::Interpreter::new_with_sandbox(&sandbox);
        let mut builtin_names = HashSet::new();
        interp.global_env.iter_bindings(|spur, _| {
            builtin_names.insert(sema_core::resolve(spur));
        });

        BackendState {
            builtin_names,
            documents: HashMap::new(),
            cached_user_defs: HashMap::new(),
            builtin_docs: builtin_docs::BuiltinDocs::load(),
            import_cache: HashMap::new(),
            cached_parses: HashMap::new(),
            sema_binary: default_sema_binary(),
            run_sandbox_mode: "off".to_string(),
        }
    }

    /// Lightweight constructor with only documents — for subprocess dispatch threads.
    pub(crate) fn new_without_builtins(
        documents: HashMap<String, String>,
        sema_binary: String,
        run_sandbox_mode: String,
    ) -> Self {
        BackendState {
            builtin_names: HashSet::new(),
            documents,
            cached_user_defs: HashMap::new(),
            builtin_docs: builtin_docs::BuiltinDocs::empty(),
            import_cache: HashMap::new(),
            cached_parses: HashMap::new(),
            sema_binary,
            run_sandbox_mode,
        }
    }

    /// Maximum number of entries in the import cache. Prevents unbounded
    /// memory growth when scanning large workspaces.
    const MAX_IMPORT_CACHE_SIZE: usize = 500;

    /// Get or refresh the cached parse result for an imported file.
    pub(crate) fn get_import_cache(&mut self, path: &Path) -> Option<&ImportCache> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;

        // Check if cache is still valid
        if let Some(cached) = self.import_cache.get(path) {
            if cached.mtime == mtime {
                return self.import_cache.get(path);
            }
        }

        // Evict oldest entries when at capacity (by arbitrary key order —
        // not true LRU, but prevents unbounded growth cheaply).
        if self.import_cache.len() >= Self::MAX_IMPORT_CACHE_SIZE {
            let keys_to_remove: Vec<PathBuf> = self
                .import_cache
                .keys()
                .take(Self::MAX_IMPORT_CACHE_SIZE / 10)
                .cloned()
                .collect();
            for key in keys_to_remove {
                self.import_cache.remove(&key);
            }
        }

        // Read and parse the file
        let text = std::fs::read_to_string(path).ok()?;
        let (ast, span_map, symbol_spans) = sema_reader::read_many_with_symbol_spans(&text).ok()?;
        // Drop quoted (data) symbol occurrences (see filter_quoted_symbol_spans).
        let symbol_spans = filter_quoted_symbol_spans(&ast, &span_map, symbol_spans);
        let scope_tree = scope::ScopeTree::build(&ast, &span_map, &symbol_spans);

        self.import_cache.insert(
            path.to_path_buf(),
            ImportCache {
                ast,
                span_map,
                symbol_spans,
                scope_tree,
                source: text,
                mtime,
            },
        );
        self.import_cache.get(path)
    }

    /// Index every top-level definition across open documents: name → (uri, form range, name range).
    pub(crate) fn def_index(&self) -> std::collections::HashMap<String, (Url, Range, Range)> {
        let mut index = std::collections::HashMap::new();
        for (uri_str, cached) in &self.cached_parses {
            let uri = match Url::parse(uri_str) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let lines: Vec<&str> = cached.source.lines().collect();
            for expr in &cached.ast {
                if let Some((name, form_range, name_range)) =
                    def_of_form(expr, &cached.span_map, &cached.symbol_spans, &lines)
                {
                    index
                        .entry(name)
                        .or_insert((uri.clone(), form_range, name_range));
                }
            }
        }
        index
    }

    pub(crate) fn call_hierarchy_item(
        name: &str,
        uri: &Url,
        range: Range,
        selection_range: Range,
    ) -> CallHierarchyItem {
        CallHierarchyItem {
            name: name.to_string(),
            kind: SymbolKind::FUNCTION,
            tags: None,
            detail: None,
            uri: uri.clone(),
            range,
            selection_range,
            data: None,
        }
    }
}
