//! Completion (`textDocument/completion`) and completion resolve.

use tower_lsp::lsp_types::*;

use crate::builtin_docs;
use crate::helpers::*;
use crate::state::BackendState;

impl BackendState {
    pub(crate) fn handle_complete(&self, uri: &Url, position: &Position) -> Vec<CompletionItem> {
        let uri_str = uri.as_str();
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
                let detail = user_ast.and_then(|ast| extract_params_from_ast(ast, name));
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail,
                    data: Some(serde_json::Value::String(uri_str.to_string())),
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
        let uri_hint = item.data.as_ref().and_then(|v| v.as_str());
        if let Some(sig) = self.user_definition_signature(&item.label, uri_hint) {
            let mut value = format!("```sema\n{sig}\n```");
            if let Some(doc) = self.user_definition_docstring(&item.label, uri_hint) {
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
    fn user_definition_docstring(&self, name: &str, uri_hint: Option<&str>) -> Option<String> {
        if let Some(uri) = uri_hint {
            if let Some(cached) = self.cached_parses.get(uri) {
                if let Some(doc) = extract_docstring_from_ast(&cached.ast, name) {
                    return Some(doc);
                }
            }
        }
        for cached in self.cached_parses.values() {
            if let Some(doc) = extract_docstring_from_ast(&cached.ast, name) {
                return Some(doc);
            }
        }
        None
    }

    /// Build a one-line signature `(name params...)` for a user-defined function, preferring the
    /// hinted document and falling back to any open document.
    fn user_definition_signature(&self, name: &str, uri_hint: Option<&str>) -> Option<String> {
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
            if let Some(cached) = self.cached_parses.get(uri) {
                if let Some(params) = extract_params_from_ast(&cached.ast, name) {
                    return Some(render(params));
                }
            }
        }
        for cached in self.cached_parses.values() {
            if let Some(params) = extract_params_from_ast(&cached.ast, name) {
                return Some(render(params));
            }
        }
        None
    }
}
