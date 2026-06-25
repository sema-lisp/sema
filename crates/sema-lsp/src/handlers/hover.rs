//! Hover (`textDocument/hover`).

use std::path::Path;

use tower_lsp::lsp_types::*;

use crate::builtin_docs;
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

        // A user definition in this file shadows a builtin of the same name, so
        // check user definitions FIRST: hovering a redefined `map` should show
        // the user's signature, not the builtin's doc.
        if let Some(cached) = self.cached_parses.get(uri_str) {
            // Only names are used here (ranges discarded), so the line context
            // is irrelevant — pass &[] to skip UTF-16 mapping.
            let defs =
                user_definitions_from_ast(&cached.ast, &cached.span_map, &cached.symbol_spans, &[]);
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

        // Phase 3c: Check imported modules for hover info
        {
            let cached = self.cached_parses.get(uri_str)?;
            let import_paths = import_paths_from_ast(&cached.ast);
            for path_str in &import_paths {
                let resolved = match resolve_import_path(uri, path_str) {
                    Some(p) if p.exists() => p,
                    _ => continue,
                };
                let import_cached = match self.get_import_cache(&resolved) {
                    Some(c) => c,
                    None => continue,
                };
                // Names only; ranges discarded — &[] skips UTF-16 mapping.
                let target_defs = user_definitions_from_ast(
                    &import_cached.ast,
                    &import_cached.span_map,
                    &import_cached.symbol_spans,
                    &[],
                );
                if target_defs.iter().any(|(n, _)| n == &symbol) {
                    let module_name = Path::new(path_str)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(path_str);
                    let mut hover_text = format!("```sema\n({symbol}");
                    if let Some(params) = extract_params_from_ast(&import_cached.ast, &symbol) {
                        hover_text.push(' ');
                        hover_text.push_str(&params);
                    }
                    hover_text.push_str(&format!(")\n```\n\n*Imported from `{module_name}`*"));
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

        None
    }
}
