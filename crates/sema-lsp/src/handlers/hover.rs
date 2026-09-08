//! Hover (`textDocument/hover`).

use tower_lsp::lsp_types::*;

use crate::builtin_docs;
use crate::definitions::*;
use crate::helpers::*;
use crate::state::BackendState;

impl BackendState {
    pub(crate) fn handle_hover(&mut self, uri: &Url, position: &Position) -> Option<Hover> {
        let uri_str = uri.as_str();
        let text = self.documents.get(uri_str)?;
        let line_idx = position.line as usize;
        let line = line_at(text, line_idx)?;
        let byte_offset = utf16_to_byte_offset(line, position.character);
        let symbol = extract_symbol_at(line, byte_offset).to_string();
        if symbol.is_empty() {
            return None;
        }

        let cached = self.cached_parses.get(uri_str)?;
        let sema_line = position.line as usize + 1;
        let sema_col = utf16_to_char_col(line, position.character as usize);
        if !cached
            .symbol_spans
            .iter()
            .any(|(name, span)| name == &symbol && span.contains_pos(sema_line, sema_col))
        {
            return None;
        }
        if let Some(resolved) = cached.scope_tree.resolve_at(&symbol, sema_line, sema_col) {
            if !resolved.is_top_level {
                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: format!("```sema\n{symbol}\n```\n\n*Local binding*"),
                    }),
                    range: None,
                });
            }
        }

        // A user definition in this file shadows a builtin of the same name, so
        // check user definitions FIRST: hovering a redefined `map` should show
        // the user's signature, not the builtin's doc.
        if uri.to_file_path().is_err() {
            if let Some(cached) = self.cached_parses.get(uri_str) {
                // Only names are used here (ranges discarded), so the line context
                // is irrelevant — pass &[] to skip UTF-16 mapping.
                let defs = user_definitions_from_ast(
                    &cached.ast,
                    &cached.span_map,
                    &cached.symbol_spans,
                    &[],
                );
                if defs.iter().any(|(name, _)| name == &symbol) {
                    let mut hover_text = format!("```sema\n({symbol}");
                    if let Some(params) = extract_params_from_ast(&cached.ast, &symbol) {
                        hover_text.push(' ');
                        hover_text.push_str(&params);
                    }
                    hover_text.push_str(")\n```\n\n");
                    if let Some(docstring) = extract_docstring_from_ast(&cached.ast, &symbol) {
                        hover_text.push_str(&docstring);
                        hover_text.push_str("\n\n");
                    }
                    hover_text.push_str("*User-defined*");
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: hover_text,
                        }),
                        range: None,
                    });
                }
            }
        }

        // Imported and re-exported definitions shadow builtins.
        self.prepare_navigation_index(uri);
        if let Some((file, definition)) = self.visible_indexed_definition(uri, &symbol, *position) {
            let mut hover_text = format!("```sema\n({symbol}");
            if let Some(params) = &definition.params {
                hover_text.push(' ');
                hover_text.push_str(params);
            }
            hover_text.push_str(")\n```\n\n");
            let is_current_file = uri
                .to_file_path()
                .ok()
                .is_some_and(|path| canonicalize_or_raw(&path) == file.path);
            if is_current_file {
                if let Some(docstring) = &definition.docstring {
                    hover_text.push_str(docstring);
                    hover_text.push_str("\n\n");
                }
                hover_text.push_str("*User-defined*");
            } else {
                let module_name = file
                    .path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("module");
                hover_text.push_str(&format!("*Imported from `{module_name}`*"));
            }
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: hover_text,
                }),
                range: None,
            });
        }

        // Builtin docs (rendered markdown), for names the user hasn't redefined.
        if let Some(e) = self.builtin_docs.get(symbol.as_str()) {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: builtin_docs::render_markdown(e),
                }),
                range: None,
            });
        }

        // Check if it's a known special form (without explicit doc)
        if sema_eval::SPECIAL_FORM_NAMES.contains(&symbol.as_str()) {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("```sema\n{symbol}\n```\n\n*Special form*"),
                }),
                range: None,
            });
        }

        // Check if it's a known builtin (without explicit doc)
        if self.builtin_names.contains(symbol.as_str()) {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("```sema\n{symbol}\n```\n\n*Built-in function*"),
                }),
                range: None,
            });
        }

        None
    }
}
