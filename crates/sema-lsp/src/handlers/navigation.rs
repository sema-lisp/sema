//! Navigation: goto-definition, references, document highlight, and rename.

use std::collections::HashMap;

use tower_lsp::lsp_types::*;

use crate::helpers::*;
use crate::state::BackendState;

impl BackendState {
    pub(crate) fn handle_goto_definition(
        &mut self,
        uri: &Url,
        position: &Position,
    ) -> Option<GotoDefinitionResponse> {
        let uri_str = uri.as_str();
        let cached = self.cached_parses.get(uri_str)?;

        // Phase 3a: Check if cursor is on an import/load path string
        if let Some(path_str) = import_path_from_ast(&cached.ast, &cached.span_map, position.line) {
            if let Some(resolved) = resolve_import_path(uri, &path_str) {
                if resolved.exists() {
                    let target_uri = Url::from_file_path(&resolved).ok()?;
                    return Some(GotoDefinitionResponse::Scalar(Location {
                        uri: target_uri,
                        range: Range::default(),
                    }));
                }
            }
            return None;
        }

        // Phase 3b: Check if cursor is on a user-defined symbol
        let line_idx = position.line as usize;
        let lines: Vec<&str> = cached.source.lines().collect();
        let line = lines.get(line_idx).copied()?;
        let byte_offset = utf16_to_byte_offset(line, position.character);
        let symbol = extract_symbol_at(line, byte_offset).to_string();
        if symbol.is_empty() {
            return None;
        }

        // Check scope tree for binding definition (local + top-level)
        let cached = self.cached_parses.get(uri_str)?;
        let sema_line = position.line as usize + 1;
        let sema_col = utf16_to_char_col(line, position.character as usize);
        if let Some(resolved) = cached.scope_tree.resolve_at(&symbol, sema_line, sema_col) {
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: span_to_range(&resolved.def_span, &lines),
            }));
        }

        // Phase 3c: Search imported modules for the definition
        let import_paths = import_paths_from_ast(&cached.ast);
        for path_str in &import_paths {
            let resolved = match resolve_import_path(uri, path_str) {
                Some(p) if p.exists() => p,
                _ => continue,
            };
            let cached = match self.get_import_cache(&resolved) {
                Some(c) => c,
                None => continue,
            };
            let target_lines: Vec<&str> = cached.source.lines().collect();
            let target_defs = user_definitions_from_ast(
                &cached.ast,
                &cached.span_map,
                &cached.symbol_spans,
                &target_lines,
            );
            for (name, range) in &target_defs {
                if name == &symbol {
                    if let Some(range) = range {
                        let target_uri = Url::from_file_path(&resolved).ok()?;
                        return Some(GotoDefinitionResponse::Scalar(Location {
                            uri: target_uri,
                            range: *range,
                        }));
                    }
                }
            }
        }

        // Phase 3d: Fall back to a workspace-wide search over open documents
        // and the workspace scan cache, mirroring how references and rename
        // treat top-level symbols as workspace-global. Without this, a
        // definition in a sibling file that is not explicitly imported is
        // unreachable even though the scan has already parsed it.
        let mut searched_paths = std::collections::HashSet::new();
        let mut locations = Vec::new();

        for (doc_uri_str, cached) in &self.cached_parses {
            let doc_uri = match Url::parse(doc_uri_str) {
                Ok(u) => u,
                Err(_) => continue,
            };
            if let Ok(doc_path) = doc_uri.to_file_path() {
                searched_paths.insert(canonicalize_or_raw(&doc_path));
            }
            let doc_lines: Vec<&str> = cached.source.lines().collect();
            let defs = user_definitions_from_ast(
                &cached.ast,
                &cached.span_map,
                &cached.symbol_spans,
                &doc_lines,
            );
            for (name, range) in &defs {
                if name == &symbol {
                    if let Some(range) = range {
                        locations.push(Location {
                            uri: doc_uri.clone(),
                            range: *range,
                        });
                    }
                }
            }
        }

        // Files known only from the workspace scan (import_cache); skip files
        // already covered above via their open-document parse — by canonical
        // path, since one file may be addressed under several spellings —
        // and entries whose on-disk file changed or vanished since the scan.
        for (path, import_cached) in &self.import_cache {
            if searched_paths.contains(&canonicalize_or_raw(path)) {
                continue;
            }
            if !import_cached.is_fresh(path) {
                continue;
            }
            let import_uri = match Url::from_file_path(path) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let import_lines: Vec<&str> = import_cached.source.lines().collect();
            let defs = user_definitions_from_ast(
                &import_cached.ast,
                &import_cached.span_map,
                &import_cached.symbol_spans,
                &import_lines,
            );
            for (name, range) in &defs {
                if name == &symbol {
                    if let Some(range) = range {
                        locations.push(Location {
                            uri: import_uri.clone(),
                            range: *range,
                        });
                    }
                }
            }
        }

        match locations.len() {
            0 => None,
            1 => Some(GotoDefinitionResponse::Scalar(locations.remove(0))),
            // Same top-level name defined in several files: return them all
            // (cache iteration order is arbitrary — picking one would be a
            // coin flip; clients render an array as a location picker).
            _ => Some(GotoDefinitionResponse::Array(locations)),
        }
    }

    pub(crate) fn handle_references(&self, uri: &Url, position: &Position) -> Vec<Location> {
        let uri_str = uri.as_str();
        let text = match self.documents.get(uri_str) {
            Some(t) => t,
            None => return vec![],
        };

        let lines: Vec<&str> = text.lines().collect();
        let line_idx = position.line as usize;
        let line = match lines.get(line_idx).copied() {
            Some(l) => l,
            None => return vec![],
        };
        let byte_offset = utf16_to_byte_offset(line, position.character);
        let symbol = extract_symbol_at(line, byte_offset);
        if symbol.is_empty() {
            return vec![];
        }

        // 1-indexed position for scope tree queries
        let sema_line = position.line as usize + 1;
        let sema_col = utf16_to_char_col(line, position.character as usize);

        // Check scope tree in the current document
        if let Some(cached) = self.cached_parses.get(uri_str) {
            if cached
                .scope_tree
                .is_locally_scoped(symbol, sema_line, sema_col)
            {
                // Locally scoped — only return references within this document's scope
                let refs = cached.scope_tree.find_scope_aware_references(
                    symbol,
                    sema_line,
                    sema_col,
                    &cached.symbol_spans,
                );
                return refs
                    .into_iter()
                    .map(|span| Location {
                        uri: uri.clone(),
                        range: span_to_range(&span, &lines),
                    })
                    .collect();
            }
        }

        // Top-level/global symbol — search all open documents, but skip
        // occurrences that are shadowed by local bindings in each document.
        let mut locations = Vec::new();
        let mut searched_paths = std::collections::HashSet::new();

        for (doc_uri_str, cached) in &self.cached_parses {
            let doc_uri = match Url::parse(doc_uri_str) {
                Ok(u) => u,
                Err(_) => continue,
            };
            if let Ok(doc_path) = doc_uri.to_file_path() {
                searched_paths.insert(canonicalize_or_raw(&doc_path));
            }
            let doc_lines: Vec<&str> = self
                .documents
                .get(doc_uri_str)
                .map(|t| t.lines().collect())
                .unwrap_or_default();
            for (name, span) in &cached.symbol_spans {
                if name != symbol {
                    continue;
                }
                // Only include this occurrence if it resolves to the top-level
                // definition (not shadowed by a local binding).
                match cached.scope_tree.resolve_at(name, span.line, span.col) {
                    Some(resolved) if !resolved.is_top_level => continue,
                    _ => {}
                }
                locations.push(Location {
                    uri: doc_uri.clone(),
                    range: span_to_range(span, &doc_lines),
                });
            }
        }

        // Also search workspace files not currently open (import_cache).
        // Skip files already searched via cached_parses — by canonical path,
        // since one file may be addressed under several spellings — and
        // entries whose on-disk file changed or vanished since the scan.
        for (path, import_cached) in &self.import_cache {
            if searched_paths.contains(&canonicalize_or_raw(path)) {
                continue;
            }
            if !import_cached.is_fresh(path) {
                continue;
            }
            let import_uri = match Url::from_file_path(path) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let import_lines: Vec<&str> = import_cached.source.lines().collect();
            for (name, span) in &import_cached.symbol_spans {
                if name != symbol {
                    continue;
                }
                match import_cached
                    .scope_tree
                    .resolve_at(name, span.line, span.col)
                {
                    Some(resolved) if !resolved.is_top_level => continue,
                    _ => {}
                }
                locations.push(Location {
                    uri: import_uri.clone(),
                    range: span_to_range(span, &import_lines),
                });
            }
        }

        locations
    }

    pub(crate) fn handle_document_highlight(
        &self,
        uri: &Url,
        position: &Position,
    ) -> Option<Vec<DocumentHighlight>> {
        let uri_str = uri.as_str();
        let cached = self.cached_parses.get(uri_str)?;
        let lines: Vec<&str> = cached.source.lines().collect();
        let line_idx = position.line as usize;
        let line = lines.get(line_idx).copied()?;
        let byte_offset = utf16_to_byte_offset(line, position.character);
        let symbol = extract_symbol_at(line, byte_offset);
        if symbol.is_empty() {
            return None;
        }

        let sema_line = position.line as usize + 1;
        let sema_col = utf16_to_char_col(line, position.character as usize);

        // Use scope-aware references for locally scoped symbols
        if cached
            .scope_tree
            .is_locally_scoped(symbol, sema_line, sema_col)
        {
            let refs = cached.scope_tree.find_scope_aware_references(
                symbol,
                sema_line,
                sema_col,
                &cached.symbol_spans,
            );
            let highlights: Vec<DocumentHighlight> = refs
                .into_iter()
                .map(|span| DocumentHighlight {
                    range: span_to_range(&span, &lines),
                    kind: None,
                })
                .collect();
            return if highlights.is_empty() {
                None
            } else {
                Some(highlights)
            };
        }

        // Top-level/global: all occurrences in this document that resolve to top-level
        let mut highlights = Vec::new();
        for (name, span) in &cached.symbol_spans {
            if name != symbol {
                continue;
            }
            match cached.scope_tree.resolve_at(name, span.line, span.col) {
                Some(resolved) if !resolved.is_top_level => continue,
                _ => {}
            }
            highlights.push(DocumentHighlight {
                range: span_to_range(span, &lines),
                kind: None,
            });
        }

        if highlights.is_empty() {
            None
        } else {
            Some(highlights)
        }
    }

    pub(crate) fn handle_prepare_rename(
        &self,
        uri: &Url,
        position: &Position,
    ) -> Option<PrepareRenameResponse> {
        // Find the symbol occurrence at this cursor position using cached parse
        let cached = self.cached_parses.get(uri.as_str())?;
        let lines: Vec<&str> = cached.source.lines().collect();
        let line_idx = position.line as usize;
        let line = lines.get(line_idx).copied()?;
        let byte_offset = utf16_to_byte_offset(line, position.character);
        let symbol = extract_symbol_at(line, byte_offset);
        if symbol.is_empty() {
            return None;
        }

        // Don't allow renaming builtins or special forms
        if self.builtin_names.contains(symbol) || sema_eval::SPECIAL_FORM_NAMES.contains(&symbol) {
            return None;
        }

        for (name, span) in &cached.symbol_spans {
            if name == symbol {
                let range = span_to_range(span, &lines);
                if position.line >= range.start.line
                    && position.line <= range.end.line
                    && position.character >= range.start.character
                    && position.character < range.end.character
                {
                    return Some(PrepareRenameResponse::RangeWithPlaceholder {
                        range,
                        placeholder: symbol.to_string(),
                    });
                }
            }
        }

        None
    }

    pub(crate) fn handle_rename(
        &self,
        uri: &Url,
        position: &Position,
        new_name: &str,
    ) -> Option<WorkspaceEdit> {
        let text = self.documents.get(uri.as_str())?;
        let lines: Vec<&str> = text.lines().collect();
        let line_idx = position.line as usize;
        let line = lines.get(line_idx).copied()?;
        let byte_offset = utf16_to_byte_offset(line, position.character);
        let symbol = extract_symbol_at(line, byte_offset);
        if symbol.is_empty() {
            return None;
        }

        // Don't allow renaming builtins or special forms
        if self.builtin_names.contains(symbol) || sema_eval::SPECIAL_FORM_NAMES.contains(&symbol) {
            return None;
        }

        // 1-indexed position for scope tree queries
        let sema_line = position.line as usize + 1;
        let sema_col = utf16_to_char_col(line, position.character as usize);

        let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

        // Check if the symbol is locally scoped
        if let Some(cached) = self.cached_parses.get(uri.as_str()) {
            if cached
                .scope_tree
                .is_locally_scoped(symbol, sema_line, sema_col)
            {
                // Locally scoped — only rename within this document's scope
                let refs = cached.scope_tree.find_scope_aware_references(
                    symbol,
                    sema_line,
                    sema_col,
                    &cached.symbol_spans,
                );
                let edits: Vec<TextEdit> = refs
                    .into_iter()
                    .map(|span| TextEdit {
                        range: span_to_range(&span, &lines),
                        new_text: new_name.to_string(),
                    })
                    .collect();
                if edits.is_empty() {
                    return None;
                }
                changes.insert(uri.clone(), edits);
                return Some(WorkspaceEdit {
                    changes: Some(changes),
                    document_changes: None,
                    change_annotations: None,
                });
            }
        }

        // Top-level/global symbol — rename across all documents,
        // but skip occurrences shadowed by local bindings.
        let mut searched_paths = std::collections::HashSet::new();

        for (doc_uri_str, cached) in &self.cached_parses {
            let doc_uri = match Url::parse(doc_uri_str) {
                Ok(u) => u,
                Err(_) => continue,
            };
            if let Ok(doc_path) = doc_uri.to_file_path() {
                searched_paths.insert(canonicalize_or_raw(&doc_path));
            }
            let doc_lines: Vec<&str> = self
                .documents
                .get(doc_uri_str)
                .map(|t| t.lines().collect())
                .unwrap_or_default();
            let mut edits = Vec::new();
            for (name, span) in &cached.symbol_spans {
                if name != symbol {
                    continue;
                }
                match cached.scope_tree.resolve_at(name, span.line, span.col) {
                    Some(resolved) if !resolved.is_top_level => continue,
                    _ => {}
                }
                edits.push(TextEdit {
                    range: span_to_range(span, &doc_lines),
                    new_text: new_name.to_string(),
                });
            }
            if !edits.is_empty() {
                changes.insert(doc_uri, edits);
            }
        }

        // Also rename in workspace files not currently open (import_cache).
        // Skip files already renamed via cached_parses — by canonical path,
        // since one file may be addressed under several spellings (duplicate
        // edits for one file would each be applied, corrupting it) — and
        // entries whose on-disk file changed or vanished since the scan:
        // their stale offsets would corrupt the file too, so missing that
        // file is the safe behavior.
        for (path, import_cached) in &self.import_cache {
            if searched_paths.contains(&canonicalize_or_raw(path)) {
                continue;
            }
            if !import_cached.is_fresh(path) {
                continue;
            }
            let import_uri = match Url::from_file_path(path) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let import_lines: Vec<&str> = import_cached.source.lines().collect();
            let mut edits = Vec::new();
            for (name, span) in &import_cached.symbol_spans {
                if name != symbol {
                    continue;
                }
                match import_cached
                    .scope_tree
                    .resolve_at(name, span.line, span.col)
                {
                    Some(resolved) if !resolved.is_top_level => continue,
                    _ => {}
                }
                edits.push(TextEdit {
                    range: span_to_range(span, &import_lines),
                    new_text: new_name.to_string(),
                });
            }
            if !edits.is_empty() {
                changes.insert(import_uri, edits);
            }
        }

        if changes.is_empty() {
            return None;
        }

        Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        })
    }
}
