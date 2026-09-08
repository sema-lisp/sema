//! Compact, complete semantic facts for files in configured workspace roots.
//!
//! Full parser state is useful while serving a request, but it is too large to
//! retain for every closed file. This module stores the parser-derived facts
//! that workspace features need. Entries are never evicted by a cache policy;
//! file discovery and file events add, replace, and remove them explicitly.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tower_lsp::lsp_types::{Range, SymbolKind, Url};

use crate::definitions::{
    flatten_module_forms, import_specs_from_ast, module_exports, params_of, scan_definitions,
    symbol_kind_for, ImportSpec, SYMBOL_HEADS,
};
use crate::helpers::span_to_range;
use crate::helpers::{expr_span, find_name_span, walk_values};
use crate::state::ParsedFile;

#[derive(Clone, Debug)]
pub(crate) struct IndexedDefinition {
    pub(crate) name: String,
    pub(crate) kind: SymbolKind,
    pub(crate) form_range: Range,
    pub(crate) name_range: Range,
    pub(crate) params: Option<String>,
    pub(crate) docstring: Option<String>,
    pub(crate) calls: HashMap<String, Vec<Range>>,
}

#[derive(Clone, Debug)]
pub(crate) struct IndexedFile {
    pub(crate) path: PathBuf,
    pub(crate) uri: Url,
    pub(crate) digest: [u8; 32],
    modified: Option<std::time::SystemTime>,
    disk_len: Option<u64>,
    pub(crate) definitions: Vec<IndexedDefinition>,
    pub(crate) imports: Vec<ImportSpec>,
    pub(crate) exports: Option<HashSet<String>>,
    /// Symbol occurrences which the parser's scope model resolves to the
    /// file's top level. Ranges are precomputed while source text is present.
    pub(crate) top_level_occurrences: HashMap<String, Vec<Range>>,
    pub(crate) local_names_at_occurrence: HashMap<String, Vec<(Range, HashSet<String>)>>,
    /// Export-list names refer to the module's final binding even when their
    /// declaration appears before the definition or re-exporting import.
    pub(crate) export_occurrences: Vec<Range>,
}

impl IndexedFile {
    pub(crate) fn from_parsed(path: PathBuf, parsed: &ParsedFile) -> Option<Self> {
        let uri = Url::from_file_path(&path).ok()?;
        Self::from_parsed_with_uri(path, uri, parsed)
    }

    pub(crate) fn from_parsed_with_uri(
        path: PathBuf,
        uri: Url,
        parsed: &ParsedFile,
    ) -> Option<Self> {
        let digest: [u8; 32] = Sha256::digest(parsed.source.as_bytes()).into();
        let metadata = std::fs::metadata(&path).ok();
        let lines: Vec<&str> = parsed.source.lines().collect();
        let definitions = scan_definitions(
            flatten_module_forms(&parsed.ast),
            SYMBOL_HEADS,
            &parsed.span_map,
            &parsed.symbol_spans,
            &lines,
        )
        .into_iter()
        .filter_map(|definition| {
            let form_range = definition.form_range?;
            let kind = symbol_kind_for(&definition.head, definition.is_shorthand);
            let mut calls: HashMap<String, Vec<Range>> = HashMap::new();
            walk_values(definition.body(), |expr| {
                let Some(items) = expr.as_list() else {
                    return;
                };
                let Some(name) = items.first().and_then(sema_core::Value::as_symbol) else {
                    return;
                };
                let Some(span) = expr_span(expr, &parsed.span_map) else {
                    return;
                };
                let range = find_name_span(&name, span, &parsed.symbol_spans, &lines)
                    .unwrap_or_else(|| span_to_range(span, &lines));
                calls.entry(name).or_default().push(range);
            });
            Some(IndexedDefinition {
                params: params_of(&definition),
                docstring: definition.docstring(),
                calls,
                name_range: definition.name_range.unwrap_or(form_range),
                form_range,
                kind,
                name: definition.name,
            })
        })
        .collect();

        let mut top_level_occurrences: HashMap<String, Vec<Range>> = HashMap::new();
        let mut local_names_at_occurrence: HashMap<String, Vec<(Range, HashSet<String>)>> =
            HashMap::new();
        let mut export_occurrences = Vec::new();
        walk_values(&parsed.ast, |expr| {
            let Some(items) = expr.as_list() else {
                return;
            };
            if items
                .first()
                .and_then(sema_core::Value::as_symbol)
                .as_deref()
                != Some("export")
            {
                return;
            }
            let Some(form_span) = expr_span(expr, &parsed.span_map) else {
                return;
            };
            let names: HashSet<String> = items[1..]
                .iter()
                .filter_map(sema_core::Value::as_symbol)
                .collect();
            export_occurrences.extend(
                parsed
                    .symbol_spans
                    .iter()
                    .filter(|(name, span)| {
                        names.contains(name) && form_span.contains_pos(span.line, span.col)
                    })
                    .map(|(_, span)| span_to_range(span, &lines)),
            );
        });
        for (name, span) in &parsed.symbol_spans {
            if parsed
                .scope_tree
                .resolves_to_top_level(name, span.line, span.col)
            {
                let range = span_to_range(span, &lines);
                top_level_occurrences
                    .entry(name.clone())
                    .or_default()
                    .push(range);
                let locals = parsed
                    .scope_tree
                    .visible_bindings_at(span.line, span.col)
                    .into_iter()
                    .filter_map(|(visible, binding)| {
                        (visible != *name && binding != *span).then_some(visible)
                    })
                    .collect();
                local_names_at_occurrence
                    .entry(name.clone())
                    .or_default()
                    .push((range, locals));
            }
        }

        Some(Self {
            imports: import_specs_from_ast(&parsed.ast, &parsed.span_map),
            exports: module_exports(&parsed.ast),
            path,
            uri,
            digest,
            modified: metadata.as_ref().and_then(|meta| meta.modified().ok()),
            disk_len: metadata.map(|meta| meta.len()),
            definitions,
            top_level_occurrences,
            local_names_at_occurrence,
            export_occurrences,
        })
    }

    pub(crate) fn is_current_on_disk(&self) -> bool {
        let Ok(metadata) = std::fs::metadata(&self.path) else {
            return false;
        };
        self.modified
            .zip(self.disk_len)
            .is_some_and(|(modified, len)| {
                metadata.modified().ok() == Some(modified) && metadata.len() == len
            })
    }

    pub(crate) fn digest_is_current_on_disk(&self) -> bool {
        std::fs::read(&self.path)
            .ok()
            .is_some_and(|source| <[u8; 32]>::from(Sha256::digest(source)) == self.digest)
    }
}

#[derive(Default)]
pub(crate) struct WorkspaceIndex {
    files: HashMap<PathBuf, IndexedFile>,
}

impl WorkspaceIndex {
    pub(crate) fn insert(&mut self, file: IndexedFile) {
        self.files.insert(file.path.clone(), file);
    }

    pub(crate) fn remove(&mut self, path: &Path) {
        self.files.remove(path);
    }

    pub(crate) fn retain_roots(&mut self, roots: &HashSet<PathBuf>) {
        self.files
            .retain(|path, _| roots.iter().any(|root| path.starts_with(root)));
    }

    pub(crate) fn remove_missing_files(&mut self) {
        self.files.retain(|path, _| path.is_file());
    }

    pub(crate) fn get(&self, path: &Path) -> Option<&IndexedFile> {
        self.files.get(path)
    }

    pub(crate) fn digest_matches(&self, path: &Path, digest: &[u8; 32]) -> bool {
        self.files
            .get(path)
            .is_some_and(|file| &file.digest == digest)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &IndexedFile> {
        self.files.values()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.files.len()
    }
}
