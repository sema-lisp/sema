//! Completion (`textDocument/completion`) and completion resolve.

use tower_lsp::lsp_types::*;

use crate::builtin_docs;
use crate::definitions::*;
use crate::helpers::*;
use crate::state::BackendState;

impl BackendState {
    pub(crate) fn handle_complete(
        &mut self,
        uri: &Url,
        position: &Position,
    ) -> Vec<CompletionItem> {
        let uri_str = uri.as_str();
        self.prepare_navigation_index(uri);
        let text = match self.documents.get(uri_str) {
            Some(t) => t,
            None => return vec![],
        };

        // Get the line at cursor (addresses the trailing empty line at EOF).
        let line_idx = position.line as usize;
        let line = match line_at(text, line_idx) {
            Some(l) => l,
            None => return vec![],
        };

        let byte_offset = utf16_to_byte_offset(line, position.character);
        if !completion_context_is_code(text, position) {
            return vec![];
        }
        let prefix = extract_prefix(line, byte_offset);

        let mut items = Vec::new();

        // Special forms
        for &name in sema_eval::SPECIAL_FORM_NAMES {
            if prefix.is_empty() || name.starts_with(prefix) {
                let entry = self.builtin_docs.get(name);
                items.push(CompletionItem {
                    label: name.to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    detail: entry.map(|e| builtin_docs::signature(e)),
                    documentation: entry.map(|e| {
                        Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: builtin_docs::render_markdown(e),
                        })
                    }),
                    ..Default::default()
                });
            }
        }

        // Builtins (sorted for deterministic completion order)
        let mut sorted_builtins: Vec<&String> = self.builtin_names.iter().collect();
        sorted_builtins.sort();
        for name in sorted_builtins {
            if prefix.is_empty() || name.starts_with(prefix) {
                let entry = self.builtin_docs.get(name.as_str());
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: entry.map(|e| builtin_docs::signature(e)),
                    documentation: entry.map(|e| {
                        Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: builtin_docs::render_markdown(e),
                        })
                    }),
                    ..Default::default()
                });
            }
        }

        // User definitions: use cached defs (survives syntax errors while typing).
        // Attach the function signature inline as `detail`, and carry the document uri in `data`
        // so `completionItem/resolve` can lazily render full documentation.
        let user_defs = self.cached_user_defs.get(uri_str);
        let user_ast = self.cached_parses.get(uri_str).map(|c| &c.ast);
        for name in user_defs.into_iter().flatten() {
            if prefix.is_empty() || name.starts_with(prefix) {
                let indexed = self.visible_indexed_definition(uri, name, *position);
                if uri.to_file_path().is_ok() && indexed.is_none() {
                    continue;
                }
                let detail = indexed
                    .and_then(|(_, definition)| definition.params.clone())
                    .or_else(|| {
                        uri.to_file_path()
                            .is_err()
                            .then(|| user_ast.and_then(|ast| extract_params_from_ast(ast, name)))
                            .flatten()
                    });
                let data = indexed
                    .map(|(file, definition)| completion_definition_data(file, definition))
                    .unwrap_or_else(|| serde_json::Value::String(uri_str.to_string()));
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail,
                    data: Some(data),
                    ..Default::default()
                });
            }
        }

        let import_specs = self
            .cached_parses
            .get(uri_str)
            .map(|parsed| import_specs_from_ast(&parsed.ast, &parsed.span_map))
            .unwrap_or_default();
        let mut imported_names = std::collections::HashSet::new();
        for import in import_specs
            .into_iter()
            .filter(|import| import.position <= *position)
        {
            let Some(path) = resolve_import_path(uri, &import.path) else {
                continue;
            };
            let path = canonicalize_or_raw(&path);
            let Some(indexed) = self.workspace_index.get(&path) else {
                continue;
            };
            let Ok(imported_uri) = Url::from_file_path(&path) else {
                continue;
            };
            let mut candidates: Vec<String> = import
                .selected
                .as_ref()
                .map(|selected| selected.iter().cloned().collect())
                .or_else(|| {
                    (!import.is_load)
                        .then(|| {
                            indexed
                                .exports
                                .as_ref()
                                .map(|names| names.iter().cloned().collect())
                        })
                        .flatten()
                })
                .unwrap_or_else(|| {
                    self.module_visible_names(&imported_uri)
                        .into_iter()
                        .collect()
                });
            candidates.sort();
            candidates.dedup();
            for name in candidates {
                if !(prefix.is_empty() || name.starts_with(prefix))
                    || !definition_is_visible(&name, indexed.exports.as_ref(), &import)
                    || !imported_names.insert(name.clone())
                {
                    continue;
                }
                let Some((owner, definition)) =
                    self.visible_indexed_definition(uri, &name, *position)
                else {
                    continue;
                };
                if !sema_eval::SPECIAL_FORM_NAMES.contains(&name.as_str()) {
                    items.retain(|item| item.label != name);
                }
                items.push(CompletionItem {
                    label: name,
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: definition.params.clone(),
                    data: Some(completion_definition_data(owner, definition)),
                    ..Default::default()
                });
            }
        }

        // Local bindings from scope tree
        if let Some(cached) = self.cached_parses.get(uri_str) {
            let sema_line = position.line as usize + 1;
            let sema_col = utf16_to_char_col(line, position.character as usize);
            for (name, _span) in cached.scope_tree.visible_bindings_at(sema_line, sema_col) {
                if prefix.is_empty() || name.starts_with(prefix) {
                    if !sema_eval::SPECIAL_FORM_NAMES.contains(&name.as_str()) {
                        items.retain(|item| item.label != name);
                    }
                    items.push(CompletionItem {
                        label: name,
                        kind: Some(CompletionItemKind::VARIABLE),
                        sort_text: Some("0".to_string()),
                        ..Default::default()
                    });
                }
            }
        }

        items
    }

    /// Lazily enrich a completion item with documentation (`completionItem/resolve`). Builtins and
    /// special forms already carry inline docs; this fills user-defined symbols with their signature.
    pub(crate) fn handle_completion_resolve(&self, mut item: CompletionItem) -> CompletionItem {
        if item.documentation.is_some() {
            return item;
        }
        // Builtin/special-form docs (covers any not inlined at completion time).
        if let Some(e) = self.builtin_docs.get(&item.label) {
            item.documentation = Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: builtin_docs::render_markdown(e),
            }));
            return item;
        }
        // User-defined symbol: render its signature, plus a leading-string docstring if present.
        let (uri_hint, definition_hint) = completion_definition_hint(item.data.as_ref());
        if let Some(sig) =
            self.user_definition_signature(&item.label, uri_hint.as_deref(), definition_hint)
        {
            let mut value = format!("```sema\n{sig}\n```");
            if let Some(doc) =
                self.user_definition_docstring(&item.label, uri_hint.as_deref(), definition_hint)
            {
                value.push_str("\n\n");
                value.push_str(&doc);
            }
            item.documentation = Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }));
        }
        item
    }

    /// The leading-string docstring of a user-defined function, if any (LSP-only convention).
    fn user_definition_docstring(
        &self,
        name: &str,
        uri_hint: Option<&str>,
        definition_hint: Option<Position>,
    ) -> Option<String> {
        if let Some(uri) = uri_hint {
            if definition_hint.is_none() {
                if let Some(cached) = self.cached_parses.get(uri) {
                    if let Some(doc) = extract_docstring_from_ast(&cached.ast, name) {
                        return Some(doc);
                    }
                }
            }
            if let Ok(uri) = Url::parse(uri) {
                if let Ok(path) = uri.to_file_path() {
                    if let Some(definition) = self
                        .workspace_index
                        .get(&canonicalize_or_raw(&path))
                        .and_then(|file| {
                            file.definitions.iter().find(|definition| {
                                definition.name == name
                                    && definition_hint.is_none_or(|position| {
                                        definition.form_range.start == position
                                    })
                            })
                        })
                    {
                        return definition.docstring.clone();
                    }
                }
            }
        }
        self.workspace_index
            .iter()
            .flat_map(|file| &file.definitions)
            .find(|definition| definition.name == name)
            .and_then(|definition| definition.docstring.clone())
    }

    /// Build a one-line signature `(name params...)` for a user-defined function, preferring the
    /// hinted document and falling back to any open document.
    fn user_definition_signature(
        &self,
        name: &str,
        uri_hint: Option<&str>,
        definition_hint: Option<Position>,
    ) -> Option<String> {
        let render = |params: String| {
            let inner = params
                .trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .trim()
                .to_string();
            if inner.is_empty() {
                format!("({name})")
            } else {
                format!("({name} {inner})")
            }
        };
        if let Some(uri) = uri_hint {
            if definition_hint.is_none() {
                if let Some(cached) = self.cached_parses.get(uri) {
                    if let Some(params) = extract_params_from_ast(&cached.ast, name) {
                        return Some(render(params));
                    }
                }
            }
            if let Ok(uri) = Url::parse(uri) {
                if let Ok(path) = uri.to_file_path() {
                    if let Some(params) = self
                        .workspace_index
                        .get(&canonicalize_or_raw(&path))
                        .and_then(|file| {
                            file.definitions.iter().find(|definition| {
                                definition.name == name
                                    && definition_hint.is_none_or(|position| {
                                        definition.form_range.start == position
                                    })
                            })
                        })
                        .and_then(|definition| definition.params.clone())
                    {
                        return Some(render(params));
                    }
                }
            }
        }
        self.workspace_index
            .iter()
            .flat_map(|file| &file.definitions)
            .find(|definition| definition.name == name)
            .and_then(|definition| definition.params.clone())
            .map(render)
    }
}

fn completion_definition_data(
    file: &crate::workspace::IndexedFile,
    definition: &crate::workspace::IndexedDefinition,
) -> serde_json::Value {
    serde_json::json!({
        "uri": file.uri.as_str(),
        "line": definition.form_range.start.line,
        "character": definition.form_range.start.character,
    })
}

fn completion_definition_hint(
    data: Option<&serde_json::Value>,
) -> (Option<String>, Option<Position>) {
    let Some(data) = data else {
        return (None, None);
    };
    if let Some(uri) = data.as_str() {
        return (Some(uri.to_string()), None);
    }
    let Some(uri) = data.get("uri").and_then(serde_json::Value::as_str) else {
        return (None, None);
    };
    let line = data
        .get("line")
        .and_then(serde_json::Value::as_u64)
        .and_then(|line| u32::try_from(line).ok());
    let character = data
        .get("character")
        .and_then(serde_json::Value::as_u64)
        .and_then(|character| u32::try_from(character).ok());
    (
        Some(uri.to_string()),
        line.zip(character)
            .map(|(line, character)| Position::new(line, character)),
    )
}

fn completion_context_is_code(text: &str, position: &Position) -> bool {
    let line = position.line as usize + 1;
    let source_line = line_at(text, position.line as usize).unwrap_or_default();
    let col = utf16_to_char_col(source_line, position.character as usize);
    let Ok(tokens) = sema_reader::lexer::tokenize(text) else {
        return true;
    };
    let Some(token) = tokens
        .iter()
        .find(|token| token.span.contains_pos(line, col))
    else {
        return true;
    };
    match &token.token {
        sema_reader::lexer::Token::Comment(_)
        | sema_reader::lexer::Token::String(_)
        | sema_reader::lexer::Token::Regex(_) => false,
        sema_reader::lexer::Token::FString(parts) => parts.iter().any(|part| {
            matches!(
                part,
                sema_reader::lexer::FStringPart::Expr { span, .. }
                    if span.contains_pos(line, col)
            )
        }),
        _ => true,
    }
}
