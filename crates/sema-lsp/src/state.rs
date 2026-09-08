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
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tower_lsp::lsp_types::*;

use sema_core::path::PathExt as _;
use sema_core::{Caps, Sandbox, Span, SpanMap};

use crate::builtin_docs;
use crate::byte_lru::{ByteLruCache, InsertResult};
use crate::handlers::command::EvalRunManager;
use crate::helpers::*;
use crate::scope;
use crate::workspace::{IndexedFile, WorkspaceIndex};

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
        let canonical_root = root
            .resolve_allow_missing()
            .unwrap_or_else(|_| root.to_path_buf());
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
    pub(crate) digest: [u8; 32],
}

impl ImportCache {
    fn retained_bytes(&self) -> NonZeroUsize {
        let mut bytes = self.parsed.source.capacity()
            + self.parsed.symbol_spans.capacity() * std::mem::size_of::<(String, Span)>();
        let mut pending: Vec<&sema_core::Value> = self.parsed.ast.iter().collect();
        while let Some(value) = pending.pop() {
            bytes = bytes.saturating_add(std::mem::size_of::<sema_core::Value>());
            if let Some(items) = value.as_list().or_else(|| value.as_vector()) {
                pending.extend(items);
            } else if let Some(map) = value.as_map_ref() {
                for (key, value) in map.iter() {
                    pending.push(key);
                    pending.push(value);
                }
            } else if let Some(value) = value.as_str() {
                bytes = bytes.saturating_add(value.len());
            } else if let Some(value) = value.as_bytevector() {
                bytes = bytes.saturating_add(value.len());
            } else if let Some(value) = value.as_f64_array() {
                bytes = bytes.saturating_add(std::mem::size_of_val(value));
            } else if let Some(value) = value.as_i64_array() {
                bytes = bytes.saturating_add(std::mem::size_of_val(value));
            }
        }
        NonZeroUsize::new(bytes.saturating_mul(2).saturating_add(512))
            .expect("fixed cache bookkeeping is non-zero")
    }

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
    pub(crate) workspace_roots: HashSet<PathBuf>,
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
    pub(crate) import_cache: ByteLruCache<PathBuf, ImportCache>,
    /// One request-local parse too large for the closed-file cache budget.
    /// It remains available without becoming a cache entry.
    oversized_parse: Option<(PathBuf, ImportCache)>,
    /// Complete, compact semantic facts for every discovered workspace file.
    /// This remains complete even when full closed-file parses are evicted.
    pub(crate) workspace_index: WorkspaceIndex,
    /// Cached parse results for open documents (updated on didChange).
    pub(crate) cached_parses: HashMap<String, ParsedFile>,
    /// Path to the sema binary (from initializationOptions or default).
    pub(crate) sema_binary: String,
    /// Owned subprocess evaluations, shared with their async workers.
    pub(crate) eval_runs: EvalRunManager,
    next_reconciliation: Instant,
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
#[cfg(test)]
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

impl BackendState {
    pub(crate) fn parsed_for_path(&self, path: &Path) -> Option<(Url, &ParsedFile)> {
        let canonical = canonicalize_or_raw(path);
        for (uri, parsed) in &self.cached_parses {
            let Ok(uri) = Url::parse(uri) else {
                continue;
            };
            let Ok(open_path) = uri.to_file_path() else {
                continue;
            };
            if canonicalize_or_raw(&open_path) == canonical {
                return Some((uri, parsed));
            }
        }
        let cached = self.import_cache.peek(&canonical).or_else(|| {
            self.oversized_parse
                .as_ref()
                .filter(|(path, _)| path == &canonical)
                .map(|(_, cached)| cached)
        })?;
        if !cached.is_fresh(&canonical) {
            return None;
        }
        Some((Url::from_file_path(&canonical).ok()?, &cached.parsed))
    }

    /// Ensure semantic facts and a full parse are available for `path`.
    /// An editor overlay is authoritative over the copy currently on disk.
    pub(crate) fn ensure_workspace_file(&mut self, path: &Path) {
        let canonical = canonicalize_or_raw(path);
        let open = self.cached_parses.iter().find_map(|(uri, parsed)| {
            let uri = Url::parse(uri).ok()?;
            let open_path = canonicalize_or_raw(&uri.to_file_path().ok()?);
            (open_path == canonical).then_some((uri, parsed))
        });
        if let Some((uri, parsed)) = open {
            if let Some(indexed) = IndexedFile::from_parsed_with_uri(canonical, uri, parsed) {
                self.workspace_index.insert(indexed);
            }
            return;
        }
        if self
            .workspace_index
            .get(&canonical)
            .is_some_and(crate::workspace::IndexedFile::is_current_on_disk)
        {
            return;
        }
        let _ = self.get_import_cache(&canonical);
    }

    /// Refresh stale indexed files and load the complete transitive import
    /// closure needed by module-aware navigation. Files outside workspace roots
    /// are loaded on demand and remain subject to the full-parse LRU.
    pub(crate) fn prepare_navigation_index(&mut self, requested_uri: &Url) {
        let stale: Vec<PathBuf> = self
            .workspace_index
            .iter()
            .filter(|file| !self.indexed_file_is_current(file))
            .map(|file| file.path.clone())
            .collect();
        for path in stale {
            self.ensure_workspace_file(&path);
        }

        self.ensure_navigation_closure(requested_uri);
    }

    /// Load one module's transitive imports without walking unrelated
    /// workspace files. Workspace-wide features call this only for files that
    /// contain a candidate occurrence.
    pub(crate) fn ensure_navigation_closure(&mut self, requested_uri: &Url) {
        let mut pending = vec![requested_uri.clone()];
        let mut visited = HashSet::new();
        while let Some(uri) = pending.pop() {
            let Ok(path) = uri.to_file_path() else {
                continue;
            };
            let path = canonicalize_or_raw(&path);
            if !visited.insert(path.clone()) {
                continue;
            }
            self.ensure_workspace_file(&path);
            let imports = self
                .workspace_index
                .get(&path)
                .map(|file| file.imports.clone())
                .unwrap_or_default();
            for import in imports {
                let Some(target) = crate::helpers::resolve_import_path(&uri, &import.path) else {
                    continue;
                };
                let target = canonicalize_or_raw(&target);
                self.ensure_workspace_file(&target);
                if let Ok(target_uri) = Url::from_file_path(target) {
                    pending.push(target_uri);
                }
            }
        }
    }

    pub(crate) fn new() -> Self {
        // Create a sandboxed interpreter just to harvest builtin names.
        let sandbox = Sandbox::deny(Caps::ALL);
        let interp = sema_eval::Interpreter::new_with_sandbox(&sandbox);
        let mut builtin_names = HashSet::new();
        interp.global_env.iter_bindings(|spur, _| {
            builtin_names.insert(sema_core::resolve(spur));
        });

        BackendState {
            workspace_roots: HashSet::new(),
            builtin_names,
            documents: HashMap::new(),
            cached_user_defs: HashMap::new(),
            builtin_docs: builtin_docs::BuiltinDocs::load(),
            import_cache: ByteLruCache::new(64 * 1024 * 1024),
            oversized_parse: None,
            workspace_index: WorkspaceIndex::default(),
            cached_parses: HashMap::new(),
            sema_binary: default_sema_binary(),
            eval_runs: EvalRunManager::default(),
            next_reconciliation: Instant::now() + Duration::from_secs(30),
        }
    }

    /// Lightweight constructor with only documents — for subprocess dispatch threads.
    #[cfg(test)]
    pub(crate) fn new_without_builtins(
        documents: HashMap<String, String>,
        sema_binary: String,
    ) -> Self {
        BackendState {
            workspace_roots: HashSet::new(),
            builtin_names: HashSet::new(),
            documents,
            cached_user_defs: HashMap::new(),
            builtin_docs: builtin_docs::BuiltinDocs::empty(),
            import_cache: ByteLruCache::new(64 * 1024 * 1024),
            oversized_parse: None,
            workspace_index: WorkspaceIndex::default(),
            cached_parses: HashMap::new(),
            sema_binary,
            eval_runs: EvalRunManager::default(),
            next_reconciliation: Instant::now() + Duration::from_secs(30),
        }
    }

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
                self.invalidate_disk_cache(&path);
                return None;
            }
        };

        // Read before accepting a cache hit. File-system timestamps may retain
        // their value across a rapid edit, so content identity is authoritative.
        // On failure, drop any previously cached
        // entry: it describes content that is gone, and serving it to the
        // iterating handlers would point them at stale offsets.
        let Ok(text) = std::fs::read_to_string(&path) else {
            self.invalidate_disk_cache(&path);
            return None;
        };
        let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        let has_open_overlay = self.has_open_document(&path);
        if !has_open_overlay {
            self.workspace_index
                .refresh_disk_metadata(&path, &digest, mtime, text.len() as u64);
        }
        // Closing an unsaved document leaves its overlay in the compact index.
        // Restore disk semantics even when the disk parse is already cached.
        if !has_open_overlay && !self.workspace_index.digest_matches(&path, &digest) {
            let cached = self
                .oversized_parse
                .as_ref()
                .filter(|(cached_path, cached)| cached_path == &path && cached.digest == digest)
                .map(|(_, cached)| cached)
                .or_else(|| {
                    self.import_cache
                        .peek(&path)
                        .filter(|cached| cached.digest == digest)
                });
            if let Some(indexed) =
                cached.and_then(|cached| IndexedFile::from_parsed(path.clone(), &cached.parsed))
            {
                self.workspace_index.insert(indexed);
            }
        }
        let oversized_hit = self
            .oversized_parse
            .as_ref()
            .filter(|(cached_path, cached)| cached_path == &path && cached.digest == digest)
            .is_some();
        if oversized_hit {
            if let Some((_, cached)) = self.oversized_parse.as_mut() {
                cached.mtime = mtime;
            }
            return self.oversized_parse.as_ref().map(|(_, cached)| cached);
        }
        if self
            .import_cache
            .peek(&path)
            .is_some_and(|cached| cached.digest == digest)
        {
            return self.import_cache.get_mut(&path).map(|cached| {
                cached.mtime = mtime;
                &*cached
            });
        }
        let Ok((ast, span_map, symbol_spans)) = sema_reader::read_many_with_symbol_spans(&text)
        else {
            self.invalidate_disk_cache(&path);
            return None;
        };
        // Drop quoted (data) symbol occurrences (see filter_quoted_symbol_spans).
        let symbol_spans = filter_quoted_symbol_spans(&ast, &span_map, symbol_spans, &text);
        let scope_tree = scope::ScopeTree::build(&ast, &span_map, &symbol_spans);

        let entry = ImportCache {
            parsed: ParsedFile {
                ast,
                span_map,
                symbol_spans,
                scope_tree,
                source: text,
            },
            mtime,
            digest,
        };
        if !has_open_overlay && !self.workspace_index.digest_matches(&path, &digest) {
            if let Some(indexed) = IndexedFile::from_parsed(path.clone(), &entry.parsed) {
                self.workspace_index.insert(indexed);
            }
        }
        let retained_bytes = entry.retained_bytes();
        self.oversized_parse = None;
        match self
            .import_cache
            .insert(path.clone(), entry, retained_bytes)
        {
            InsertResult::Cached { evicted } => drop(evicted),
            InsertResult::Rejected { key, value } => {
                self.oversized_parse = Some((key, value));
                return self.oversized_parse.as_ref().map(|(_, cached)| cached);
            }
        }
        self.import_cache.get(&path)
    }

    fn has_open_document(&self, path: &Path) -> bool {
        self.cached_parses.keys().any(|uri| {
            Url::parse(uri)
                .ok()
                .and_then(|uri| uri.to_file_path().ok())
                .is_some_and(|open_path| canonicalize_or_raw(&open_path) == path)
        })
    }

    fn invalidate_disk_cache(&mut self, path: &PathBuf) {
        self.import_cache.remove(path);
        if self
            .oversized_parse
            .as_ref()
            .is_some_and(|(cached_path, _)| cached_path == path)
        {
            self.oversized_parse = None;
        }
        // Disk notifications do not change the editor's authoritative text.
        if !self.has_open_document(path) {
            self.workspace_index.remove(path);
        }
    }

    /// Refresh files discovered by the workspace scan after they change on
    /// disk. A scan is incremental and may finish long before the next editor
    /// request; refreshing stale entries here keeps workspace navigation and
    /// symbols useful without waiting for the client to restart the server.
    pub(crate) fn refresh_stale_import_cache(&mut self) {
        let stale_paths: Vec<PathBuf> = self
            .import_cache
            .iter()
            .filter(|(path, cached)| !cached.is_fresh(path))
            .map(|(path, _)| path.clone())
            .collect();
        for path in stale_paths {
            let _ = self.get_import_cache(&path);
        }
    }

    /// Reconcile the compact index by content identity. This catches a missed
    /// watcher event even when a same-size rewrite preserves a coarse mtime.
    pub(crate) fn refresh_stale_workspace_index(&mut self) {
        let stale_paths: Vec<PathBuf> = self
            .workspace_index
            .iter()
            .filter(|file| {
                !self.has_open_document(&file.path)
                    && (!file.is_current_on_disk() || !file.digest_is_current_on_disk())
            })
            .map(|file| file.path.clone())
            .collect();
        for path in stale_paths {
            let _ = self.get_import_cache(&path);
        }
    }

    /// Schedule a complete discovery pass at a low frequency. Client watcher
    /// notifications remain the fast path; reconciliation repairs missed
    /// create/delete/change notifications and clients without file watching.
    pub(crate) fn take_reconciliation_roots(&mut self) -> Vec<PathBuf> {
        if Instant::now() < self.next_reconciliation {
            return Vec::new();
        }
        self.next_reconciliation = Instant::now() + Duration::from_secs(30);
        self.refresh_stale_import_cache();
        self.refresh_stale_workspace_index();
        self.workspace_roots.iter().cloned().collect()
    }

    pub(crate) fn add_workspace_root(&mut self, root: &Path) {
        self.workspace_roots.insert(canonicalize_or_raw(root));
    }

    pub(crate) fn remove_workspace_root(&mut self, root: &Path) {
        self.workspace_roots.remove(&canonicalize_or_raw(root));
        self.import_cache.retain(|path, _| {
            self.workspace_roots
                .iter()
                .any(|workspace_root| path.starts_with(workspace_root))
        });
        if self.oversized_parse.as_ref().is_some_and(|(path, _)| {
            !self
                .workspace_roots
                .iter()
                .any(|workspace_root| path.starts_with(workspace_root))
        }) {
            self.oversized_parse = None;
        }
        self.workspace_index.retain_roots(&self.workspace_roots);
    }

    pub(crate) fn apply_workspace_file_change(&mut self, change: &FileEvent) {
        let Ok(path) = change.uri.to_file_path() else {
            return;
        };
        let path = canonicalize_or_raw(&path);
        if change.typ == FileChangeType::DELETED {
            self.invalidate_disk_cache(&path);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("sema") {
            let _ = self.get_import_cache(&path);
        }
    }

    pub(crate) fn index_open_document(&mut self, uri: &Url) {
        let Ok(path) = uri.to_file_path() else {
            return;
        };
        let path = canonicalize_or_raw(&path);
        if let Some(parsed) = self.cached_parses.get(uri.as_str()) {
            if let Some(indexed) = IndexedFile::from_parsed_with_uri(path, uri.clone(), parsed) {
                self.workspace_index.insert(indexed);
            }
        }
    }

    pub(crate) fn indexed_file_is_current(&self, file: &crate::workspace::IndexedFile) -> bool {
        self.has_open_document(&file.path) || file.is_current_on_disk()
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
        for file in self.workspace_index.iter() {
            if !self.indexed_file_is_current(file) {
                continue;
            }
            for definition in &file.definitions {
                index.entry(definition.name.clone()).or_insert((
                    file.uri.clone(),
                    definition.form_range,
                    definition.name_range,
                ));
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
