//! Navigation: goto-definition, references, document highlight, and rename.

use std::collections::HashMap;

use tower_lsp::lsp_types::*;

use crate::definitions::*;
use crate::helpers::*;
use crate::state::BackendState;

impl BackendState {
    /// Every occurrence of `symbol` across the workspace that resolves to a
    /// top-level binding (not shadowed by a local one) — the shared core of
    /// `handle_references`'s and `handle_rename`'s workspace-wide branch.
    fn workspace_top_level_occurrences(&self, symbol: &str) -> Vec<(Url, Range)> {
        let mut out = Vec::new();
        for wf in self.iter_workspace_files() {
            let lines = wf.lines();
            for (name, span) in wf.symbol_spans {
                if name != symbol
                    || !wf
                        .scope_tree
                        .resolves_to_top_level(name, span.line, span.col)
                {
                    continue;
                }
                out.push((wf.uri.clone(), span_to_range(span, &lines)));
            }
        }
        out
    }

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
            // A path that can't become a URL can't be jumped to — skip this
            // import and keep searching the rest, instead of aborting the
            // whole goto-definition on one bad import.
            let Ok(target_uri) = Url::from_file_path(&resolved) else {
                continue;
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
        let mut locations = Vec::new();
        for wf in self.iter_workspace_files() {
            let lines = wf.lines();
            let defs = user_definitions_from_ast(wf.ast, wf.span_map, wf.symbol_spans, &lines);
            for (name, range) in &defs {
                if name == &symbol {
                    if let Some(range) = range {
                        locations.push(Location {
                            uri: wf.uri.clone(),
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
            if let Some(refs) = cached.scope_tree.locally_scoped_occurrences(
                symbol,
                sema_line,
                sema_col,
                &cached.symbol_spans,
            ) {
                // Locally scoped — only return references within this document's scope
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
        self.workspace_top_level_occurrences(symbol)
            .into_iter()
            .map(|(uri, range)| Location { uri, range })
            .collect()
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
        if let Some(refs) = cached.scope_tree.locally_scoped_occurrences(
            symbol,
            sema_line,
            sema_col,
            &cached.symbol_spans,
        ) {
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
        let highlights: Vec<DocumentHighlight> = cached
            .symbol_spans
            .iter()
            .filter(|(name, span)| {
                name == symbol
                    && cached
                        .scope_tree
                        .resolves_to_top_level(name, span.line, span.col)
            })
            .map(|(_, span)| DocumentHighlight {
                range: span_to_range(span, &lines),
                kind: None,
            })
            .collect();

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
                // End is inclusive: extract_symbol_at treats a cursor at the
                // end of a token as on-symbol (the following char is a
                // non-symbol char by token-boundary definition), and rename
                // accepts that position — prepare must agree.
                if position.line >= range.start.line
                    && position.line <= range.end.line
                    && position.character >= range.start.character
                    && position.character <= range.end.character
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
            if let Some(refs) = cached.scope_tree.locally_scoped_occurrences(
                symbol,
                sema_line,
                sema_col,
                &cached.symbol_spans,
            ) {
                // Locally scoped — only rename within this document's scope
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
        for (uri, range) in self.workspace_top_level_occurrences(symbol) {
            changes.entry(uri).or_default().push(TextEdit {
                range,
                new_text: new_name.to_string(),
            });
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
