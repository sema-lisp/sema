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
use crate::definitions::*;
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
            // Type-check through symlinks (`DirEntry::metadata` does not
            // traverse them, which would skip symlinked dirs and files);
            // cycles are prevented by the canonical-path visited set below.
            let path = entry.path();
            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue, // broken symlink or unreadable
            };
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

/// A parsed source file's cached semantic shape, shared by open documents and
/// the import cache so a new parsed-file consumer only needs to add a field
/// here, not in three places.
pub(crate) struct ParsedFile {
    pub(crate) ast: Vec<sema_core::Value>,
    pub(crate) span_map: SpanMap,
    pub(crate) symbol_spans: Vec<(String, Span)>,
    pub(crate) scope_tree: scope::ScopeTree,
    /// Source text, retained so cross-file ranges can be mapped from char
    /// columns to UTF-16 code units (LSP `Position`). See `span_to_range`.
    pub(crate) source: String,
}

/// Cached parse result for an imported file.
pub(crate) struct ImportCache {
    pub(crate) parsed: ParsedFile,
    /// Modification time when we last read the file.
    pub(crate) mtime: std::time::SystemTime,
}

impl ImportCache {
    /// Whether this entry still matches the file on disk. Handlers that
    /// iterate the cache directly (references, rename, workspace symbols,
    /// the goto-definition workspace fallback) must skip entries that are
    /// not fresh: their spans index content that no longer exists, and a
    /// rename edit built from them would corrupt the file. Missing metadata
    /// counts as stale — a deleted file has nothing to point into.
    pub(crate) fn is_fresh(&self, path: &Path) -> bool {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|mtime| mtime == self.mtime)
            .unwrap_or(false)
    }
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
    pub(crate) cached_parses: HashMap<String, ParsedFile>,
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

/// One workspace file as seen by a workspace-wide search (references, rename,
/// goto-definition's fallback, workspace symbols, call hierarchy): either an
/// open document or a still-fresh scanned file, uniformly. See
/// [`BackendState::iter_workspace_files`].
pub(crate) struct WorkspaceFile<'a> {
    pub(crate) uri: Url,
    pub(crate) parsed: &'a ParsedFile,
}

impl<'a> WorkspaceFile<'a> {
    pub(crate) fn lines(&self) -> Vec<&'a str> {
        self.parsed.source.lines().collect()
    }

    /// The file's base name without extension, for the "defined in X"
    /// attribution shown by hover/signature-help.
    pub(crate) fn stem(&self) -> String {
        // `to_file_path()` percent-decodes (unlike `uri.path()` directly) —
        // needed so a scanned file with a space or other escaped character
        // in its name (`my%20file.sema`) attributes as "my file", not
        // "my%20file".
        self.uri
            .to_file_path()
            .ok()
            .and_then(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
            .unwrap_or_default()
    }
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
        // One canonical key per file: import resolution yields un-normalized
        // paths (`a/../lib.sema`), and clients may address a file through a
        // symlinked root; distinct keys for the same file would duplicate
        // results in every handler that iterates this map.
        let path = canonicalize_or_raw(path);
        let mtime = match std::fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(mtime) => mtime,
            Err(_) => {
                // File deleted or unreadable — drop any stale entry so the
                // iterating handlers stop seeing it.
                self.import_cache.remove(&path);
                return None;
            }
        };

        // Check if cache is still valid
        if let Some(cached) = self.import_cache.get(&path) {
            if cached.mtime == mtime {
                return self.import_cache.get(&path);
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

        // Read and parse the file. On failure, drop any previously cached
        // entry: it describes content that is gone, and serving it to the
        // iterating handlers would point them at stale offsets.
        let Ok(text) = std::fs::read_to_string(&path) else {
            self.import_cache.remove(&path);
            return None;
        };
        let Ok((ast, span_map, symbol_spans)) = sema_reader::read_many_with_symbol_spans(&text)
        else {
            self.import_cache.remove(&path);
            return None;
        };
        // Drop quoted (data) symbol occurrences (see filter_quoted_symbol_spans).
        let symbol_spans = filter_quoted_symbol_spans(&ast, &span_map, symbol_spans);
        let scope_tree = scope::ScopeTree::build(&ast, &span_map, &symbol_spans);

        self.import_cache.insert(
            path.clone(),
            ImportCache {
                parsed: ParsedFile {
                    ast,
                    span_map,
                    symbol_spans,
                    scope_tree,
                    source: text,
                },
                mtime,
            },
        );
        self.import_cache.get(&path)
    }

    /// Yield every workspace file once, open documents first: other open
    /// documents (from `cached_parses`), then still-fresh scanned files not
    /// already covered by an open document (from `import_cache`, dedup'd by
    /// canonical path). Every workspace-wide search (references, rename,
    /// goto-definition's fallback, workspace symbols, call hierarchy) must
    /// apply this "open wins, stale scans skipped" rule identically — this
    /// is the one place it's implemented, so a bespoke reimplementation at a
    /// new call site can't get the dedup or freshness check subtly wrong.
    pub(crate) fn iter_workspace_files(&self) -> impl Iterator<Item = WorkspaceFile<'_>> + '_ {
        // Computed eagerly (not interleaved with the `open` iterator below)
        // so `scanned`'s filter always sees the complete set.
        let open_paths: HashSet<PathBuf> = self
            .cached_parses
            .keys()
            .filter_map(|uri_str| Url::parse(uri_str).ok())
            .filter_map(|u| u.to_file_path().ok())
            .map(|p| canonicalize_or_raw(&p))
            .collect();

        let open = self.cached_parses.iter().filter_map(|(uri_str, cached)| {
            Url::parse(uri_str).ok().map(|uri| WorkspaceFile {
                uri,
                parsed: cached,
            })
        });

        let scanned = self.import_cache.iter().filter_map(move |(path, ic)| {
            if open_paths.contains(&canonicalize_or_raw(path)) || !ic.is_fresh(path) {
                return None;
            }
            Url::from_file_path(path).ok().map(|uri| WorkspaceFile {
                uri,
                parsed: &ic.parsed,
            })
        });

        open.chain(scanned)
    }

    /// Search the whole workspace for a top-level definition of `symbol`:
    /// other open documents first, then still-fresh scanned files. Skips
    /// `current_uri`: its definitions were already consulted by the caller.
    /// Returns the defining file's AST (for signature/docstring extraction)
    /// plus a short display name (file stem) for attribution.
    pub(crate) fn find_workspace_definition(
        &self,
        current_uri: &Url,
        symbol: &str,
    ) -> Option<(&[sema_core::Value], String)> {
        self.iter_workspace_files().find_map(|wf| {
            if &wf.uri == current_uri {
                return None;
            }
            // Names only; ranges discarded — &[] skips UTF-16 mapping.
            let defs = user_definitions_from_ast(
                &wf.parsed.ast,
                &wf.parsed.span_map,
                &wf.parsed.symbol_spans,
                &[],
            );
            if defs.iter().any(|(name, _)| name == symbol) {
                Some((wf.parsed.ast.as_slice(), wf.stem()))
            } else {
                None
            }
        })
    }

    /// Index every top-level definition across open documents and still-fresh
    /// scanned workspace files: name → (uri, form range, name range). Open
    /// documents are inserted first, so they win over a scanned entry for the
    /// same name (`or_insert`). Uses `SYMBOL_HEADS`, not `DEFINITION_HEADS` —
    /// this index backs call hierarchy (`handle_call_hierarchy_prepare`),
    /// which must resolve a `defworkflow` the same way document symbols do,
    /// even though `defworkflow` isn't a real binding (see `DEFINITION_HEADS`'
    /// doc comment) — it's still a valid call-hierarchy root/target.
    pub(crate) fn def_index(&self) -> std::collections::HashMap<String, (Url, Range, Range)> {
        let mut index = std::collections::HashMap::new();
        for wf in self.iter_workspace_files() {
            let lines = wf.lines();
            for m in scan_definitions(
                flatten_module_forms(&wf.parsed.ast),
                SYMBOL_HEADS,
                &wf.parsed.span_map,
                &wf.parsed.symbol_spans,
                &lines,
            ) {
                // A definition form with no span (reader error-recovery) has
                // nothing to point at — skip it rather than indexing a bogus
                // 0:0 range that would win the `or_insert` race against the
                // real definition elsewhere.
                let Some(form_range) = m.form_range else {
                    continue;
                };
                let name_range = m.name_range.unwrap_or(form_range);
                index
                    .entry(m.name)
                    .or_insert((wf.uri.clone(), form_range, name_range));
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
