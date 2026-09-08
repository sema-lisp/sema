//! Navigation: goto-definition, references, document highlight, and rename.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tower_lsp::lsp_types::*;

use crate::definitions::*;
use crate::helpers::*;
use crate::state::BackendState;

/// The canonical identity of a source module. File URIs use their resolved
/// filesystem path so an open symlink and a scanned real path remain one
/// module; non-file documents retain their URI identity.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ModuleIdentity {
    File(PathBuf),
    Uri(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GlobalBinding {
    module: ModuleIdentity,
    definition: Position,
}

fn module_identity(uri: &Url) -> ModuleIdentity {
    uri.to_file_path()
        .map(|path| ModuleIdentity::File(canonicalize_or_raw(&path)))
        .unwrap_or_else(|_| ModuleIdentity::Uri(uri.to_string()))
}

impl BackendState {
    /// Return the indexed definition which supplies a global binding at one
    /// source position. The caller must prepare the navigation index first.
    pub(crate) fn visible_indexed_definition(
        &self,
        uri: &Url,
        symbol: &str,
        occurrence: Position,
    ) -> Option<(
        &crate::workspace::IndexedFile,
        &crate::workspace::IndexedDefinition,
    )> {
        let binding = self.global_symbol_owner(uri, symbol, Some(occurrence))?;
        let ModuleIdentity::File(path) = binding.module else {
            return None;
        };
        let file = self.workspace_index.get(&path)?;
        let definition = file.definitions.iter().find(|definition| {
            definition.name == symbol && definition.form_range.start == binding.definition
        })?;
        Some((file, definition))
    }

    /// Names installed after a module and its transitive imports finish
    /// evaluating. The navigation index must be prepared before this query.
    pub(crate) fn module_visible_names(&self, uri: &Url) -> HashSet<String> {
        self.module_visible_names_inner(uri, &mut HashSet::new())
    }

    fn module_visible_names_inner(
        &self,
        uri: &Url,
        visiting: &mut HashSet<ModuleIdentity>,
    ) -> HashSet<String> {
        let identity = module_identity(uri);
        if !visiting.insert(identity) {
            return HashSet::new();
        }
        let Ok(path) = uri.to_file_path() else {
            return HashSet::new();
        };
        let path = canonicalize_or_raw(&path);
        let Some(file) = self.workspace_index.get(&path) else {
            return HashSet::new();
        };
        let mut names: HashSet<String> = file
            .definitions
            .iter()
            .map(|definition| definition.name.clone())
            .collect();
        for import in &file.imports {
            let Some(target) = resolve_import_path(uri, &import.path) else {
                continue;
            };
            let target = canonicalize_or_raw(&target);
            let Ok(target_uri) = Url::from_file_path(&target) else {
                continue;
            };
            let target_exports = self
                .workspace_index
                .get(&target)
                .and_then(|target| target.exports.as_ref());
            names.extend(
                self.module_visible_names_inner(&target_uri, visiting)
                    .into_iter()
                    .filter(|name| definition_is_visible(name, target_exports, import)),
            );
        }
        visiting.remove(&module_identity(uri));
        names
    }

    /// Resolve a global symbol to the source module that owns its binding.
    ///
    /// A module owns its own definitions. Otherwise a symbol can only come
    /// from an explicit import or load. Import visibility follows the module's
    /// export declaration and a selective-import list; load evaluates the
    /// target into the caller's environment and therefore exposes all of its
    /// bindings. Chasing the target recursively handles re-exporting modules.
    fn global_symbol_owner(
        &self,
        uri: &Url,
        symbol: &str,
        occurrence: Option<Position>,
    ) -> Option<GlobalBinding> {
        self.global_symbol_owner_inner(uri, symbol, occurrence, &mut HashSet::new())
    }

    fn global_symbol_owner_inner(
        &self,
        uri: &Url,
        symbol: &str,
        occurrence: Option<Position>,
        visiting: &mut HashSet<ModuleIdentity>,
    ) -> Option<GlobalBinding> {
        let identity = module_identity(uri);
        if !visiting.insert(identity.clone()) {
            return None;
        }

        let (definition_positions, imports) = match &identity {
            ModuleIdentity::File(path) if self.workspace_index.get(path).is_some() => {
                let file = self.workspace_index.get(path)?;
                (
                    file.definitions
                        .iter()
                        .filter(|definition| definition.name == symbol)
                        .map(|definition| definition.form_range.start)
                        .collect::<Vec<_>>(),
                    file.imports.clone(),
                )
            }
            _ => {
                let parsed = if let Some(parsed) = self.cached_parses.get(uri.as_str()) {
                    parsed
                } else {
                    let path = uri.to_file_path().ok()?;
                    self.parsed_for_path(&path)?.1
                };
                let lines: Vec<&str> = parsed.source.lines().collect();
                let definitions = user_definitions_from_ast(
                    &parsed.ast,
                    &parsed.span_map,
                    &parsed.symbol_spans,
                    &lines,
                );
                (
                    definitions
                        .into_iter()
                        .filter(|(name, _)| name == symbol)
                        .filter_map(|(_, range)| range.map(|range| range.start))
                        .collect::<Vec<_>>(),
                    import_specs_from_ast(&parsed.ast, &parsed.span_map),
                )
            }
        };

        // Definitions, imports, and loads all update the environment in source
        // order. Walk possible providers from newest to oldest at this
        // occurrence; the first one that exports the name owns the binding.
        let mut providers: Vec<(Position, Option<ImportSpec>)> = definition_positions
            .into_iter()
            .map(|position| (position, None))
            .chain(
                imports
                    .into_iter()
                    .map(|import| (import.position, Some(import))),
            )
            .filter(|(position, _)| occurrence.is_none_or(|occurrence| *position <= occurrence))
            .collect();
        providers.sort_by_key(|(position, _)| *position);
        for (provider_position, import) in providers.into_iter().rev() {
            let Some(import) = import else {
                return Some(GlobalBinding {
                    module: identity,
                    definition: provider_position,
                });
            };
            let Some(path) = resolve_import_path(uri, &import.path) else {
                continue;
            };
            let canonical_path = canonicalize_or_raw(&path);
            let Ok(target_uri) = Url::from_file_path(&canonical_path) else {
                continue;
            };
            let exports = if let Some(file) = self.workspace_index.get(&canonical_path) {
                file.exports.clone()
            } else if let Some((_, target)) = self.parsed_for_path(&canonical_path) {
                module_exports(&target.ast)
            } else {
                continue;
            };
            if !definition_is_visible(symbol, exports.as_ref(), &import) {
                continue;
            }
            if let Some(owner) = self.global_symbol_owner_inner(&target_uri, symbol, None, visiting)
            {
                return Some(owner);
            }
        }

        None
    }

    /// Every occurrence of `symbol` whose global binding has `owner` as its
    /// canonical defining module. Same-spelling definitions in unrelated
    /// files are separate bindings, so they never enter this result.
    fn workspace_top_level_occurrences(
        &mut self,
        owner: &GlobalBinding,
        symbol: &str,
    ) -> Vec<(Url, Range)> {
        let mut out = Vec::new();
        let candidates: Vec<(Url, Vec<(Range, bool)>)> = self
            .workspace_index
            .iter()
            .filter(|file| self.indexed_file_is_current(file))
            .filter_map(|file| {
                file.top_level_occurrences.get(symbol).map(|ranges| {
                    (
                        file.uri.clone(),
                        ranges
                            .iter()
                            .map(|range| (*range, file.export_occurrences.contains(range)))
                            .collect(),
                    )
                })
            })
            .collect();
        for (uri, ranges) in candidates {
            self.ensure_navigation_closure(&uri);
            for (range, is_export) in ranges {
                if self
                    .global_symbol_owner(&uri, symbol, (!is_export).then_some(range.start))
                    .as_ref()
                    == Some(owner)
                {
                    out.push((uri.clone(), range));
                }
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
        if let Some(path_str) =
            import_path_at_cursor(&cached.source, position.line, position.character)
        {
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

        let sema_line = position.line as usize + 1;
        let sema_col = utf16_to_char_col(line, position.character as usize);
        if !cached
            .symbol_spans
            .iter()
            .any(|(name, span)| name == &symbol && span.contains_pos(sema_line, sema_col))
        {
            return None;
        }

        // Check scope tree for binding definition (local + top-level)
        let cached = self.cached_parses.get(uri_str)?;
        if let Some(resolved) = cached.scope_tree.resolve_at(&symbol, sema_line, sema_col) {
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: span_to_range(&resolved.def_span, &lines),
            }));
        }

        // Populate the direct-import entries before resolving the module
        // graph. The resolver below enforces export and selective-import
        // visibility and follows re-exports to the owning source module.
        self.prepare_navigation_index(uri);

        let owner = self.global_symbol_owner(uri, &symbol, Some(*position))?;
        if let ModuleIdentity::File(path) = &owner.module {
            if let Some(file) = self.workspace_index.get(path) {
                if let Some(definition) = file.definitions.iter().find(|definition| {
                    definition.name == symbol && definition.form_range.start == owner.definition
                }) {
                    return Some(GotoDefinitionResponse::Scalar(Location {
                        uri: file.uri.clone(),
                        range: definition.name_range,
                    }));
                }
            }
        }
        None
    }

    #[cfg(test)]
    pub(crate) fn handle_references(&mut self, uri: &Url, position: &Position) -> Vec<Location> {
        self.handle_references_with_context(uri, position, true)
    }

    pub(crate) fn handle_references_with_context(
        &mut self,
        uri: &Url,
        position: &Position,
        include_declaration: bool,
    ) -> Vec<Location> {
        let uri_str = uri.as_str();
        let text = match self.documents.get(uri_str) {
            Some(t) => t.clone(),
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
        let Some(cached) = self.cached_parses.get(uri_str) else {
            return vec![];
        };
        if !cached
            .symbol_spans
            .iter()
            .any(|(name, span)| name == symbol && span.contains_pos(sema_line, sema_col))
        {
            return vec![];
        }
        if let Some(refs) = cached.scope_tree.locally_scoped_occurrences(
            symbol,
            sema_line,
            sema_col,
            &cached.symbol_spans,
        ) {
            let definition_span = cached
                .scope_tree
                .resolve_at(symbol, sema_line, sema_col)
                .map(|resolved| resolved.def_span);
            // Locally scoped — only return references within this document's scope
            return refs
                .into_iter()
                .filter(|span| include_declaration || Some(*span) != definition_span)
                .map(|span| Location {
                    uri: uri.clone(),
                    range: span_to_range(&span, &lines),
                })
                .collect();
        }

        // Top-level/global symbol — resolve its defining module before
        // searching. Equal spellings in unrelated files are distinct
        // bindings unless explicit import/load visibility joins them.
        self.prepare_navigation_index(uri);
        let Some(owner) = self.global_symbol_owner(uri, symbol, Some(*position)) else {
            return vec![];
        };
        let mut locations: Vec<Location> = self
            .workspace_top_level_occurrences(&owner, symbol)
            .into_iter()
            .map(|(uri, range)| Location { uri, range })
            .collect();
        if !include_declaration {
            let declarations: Vec<(Url, Range)> = self
                .workspace_index
                .iter()
                .filter(|file| self.indexed_file_is_current(file))
                .flat_map(|file| {
                    file.definitions
                        .iter()
                        .filter(move |definition| definition.name == symbol)
                        .map(move |definition| (file.uri.clone(), definition.name_range))
                })
                .collect();
            locations.retain(|location| {
                !declarations
                    .iter()
                    .any(|(uri, range)| uri == &location.uri && range == &location.range)
            });
        }
        locations
    }

    pub(crate) fn handle_document_highlight(
        &mut self,
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
        if !cached
            .symbol_spans
            .iter()
            .any(|(name, span)| name == symbol && span.contains_pos(sema_line, sema_col))
        {
            return None;
        }

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

        // Top-level/global: use the same source-ordered binding identity as
        // references and rename, then keep occurrences in this document.
        let symbol = symbol.to_string();
        self.prepare_navigation_index(uri);
        let Some(owner) = self.global_symbol_owner(uri, &symbol, Some(*position)) else {
            // Builtins have no source module. Keep their document occurrences,
            // excluding quoted data and bindings that shadow the builtin.
            if !self.builtin_names.contains(&symbol) {
                return None;
            }
            let parsed = self.cached_parses.get(uri.as_str())?;
            let lines: Vec<&str> = parsed.source.lines().collect();
            let highlights: Vec<_> = parsed
                .symbol_spans
                .iter()
                .filter(|(name, span)| {
                    name == &symbol
                        && parsed
                            .scope_tree
                            .resolves_to_top_level(name, span.line, span.col)
                })
                .map(|(_, span)| span_to_range(span, &lines))
                .filter(|range| {
                    self.global_symbol_owner(uri, &symbol, Some(range.start))
                        .is_none()
                })
                .map(|range| DocumentHighlight { range, kind: None })
                .collect();
            return (!highlights.is_empty()).then_some(highlights);
        };
        let current_module = module_identity(uri);
        let highlights: Vec<DocumentHighlight> = self
            .workspace_top_level_occurrences(&owner, &symbol)
            .into_iter()
            .filter(|(occurrence_uri, _)| module_identity(occurrence_uri) == current_module)
            .map(|(_, range)| DocumentHighlight { range, kind: None })
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
        &mut self,
        uri: &Url,
        position: &Position,
        new_name: &str,
    ) -> Option<WorkspaceEdit> {
        if !is_valid_sema_symbol(new_name) {
            return None;
        }
        let text = self.documents.get(uri.as_str())?.clone();
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

        let cached = self.cached_parses.get(uri.as_str())?;
        if !cached
            .symbol_spans
            .iter()
            .any(|(name, span)| name == symbol && span.contains_pos(sema_line, sema_col))
        {
            return None;
        }

        let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

        // Check if the symbol is locally scoped
        if let Some(refs) = cached.scope_tree.locally_scoped_occurrences(
            symbol,
            sema_line,
            sema_col,
            &cached.symbol_spans,
        ) {
            let original = cached.scope_tree.resolve_at(symbol, sema_line, sema_col)?;
            if refs.iter().any(|span| {
                cached
                    .scope_tree
                    .visible_bindings_at(span.line, span.col)
                    .into_iter()
                    .any(|(name, def_span)| name == new_name && def_span != original.def_span)
            }) {
                return None;
            }
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

        self.prepare_navigation_index(uri);
        let owner = self.global_symbol_owner(uri, symbol, Some(*position))?;
        // Top-level/global symbol — only rename occurrences that resolve to
        // this exact defining module.
        let occurrences = self.workspace_top_level_occurrences(&owner, symbol);
        if occurrences.iter().any(|(occurrence_uri, _)| {
            self.workspace_index.iter().any(|file| {
                (&file.uri == occurrence_uri
                    || occurrence_uri
                        .to_file_path()
                        .ok()
                        .is_some_and(|path| canonicalize_or_raw(&path) == file.path))
                    && file
                        .definitions
                        .iter()
                        .any(|definition| definition.name == new_name)
            })
        }) {
            return None;
        }
        for (occurrence_uri, range) in &occurrences {
            let Some(file) = self.workspace_index.iter().find(|file| {
                &file.uri == occurrence_uri
                    || occurrence_uri
                        .to_file_path()
                        .ok()
                        .is_some_and(|path| canonicalize_or_raw(&path) == file.path)
            }) else {
                continue;
            };
            if file
                .local_names_at_occurrence
                .get(symbol)
                .into_iter()
                .flatten()
                .any(|(candidate_range, names)| {
                    candidate_range == range && names.contains(new_name)
                })
            {
                return None;
            }
        }
        for (uri, range) in occurrences {
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

fn is_valid_sema_symbol(name: &str) -> bool {
    let Ok(tokens) = sema_reader::lexer::tokenize(name) else {
        return false;
    };
    matches!(
        tokens.as_slice(),
        [sema_reader::lexer::SpannedToken {
            token: sema_reader::lexer::Token::Symbol(symbol),
            byte_start: 0,
            byte_end,
            ..
        }] if symbol == name && *byte_end == name.len()
    )
}
