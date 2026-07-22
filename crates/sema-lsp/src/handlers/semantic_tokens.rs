//! Semantic tokens (`textDocument/semanticTokens/full`).

use std::collections::HashSet;

use tower_lsp::lsp_types::*;

use crate::helpers::char_col_to_utf16;
use crate::state::{token_modifiers, token_types, BackendState};

impl BackendState {
    pub(crate) fn handle_semantic_tokens_full(&self, uri: &Url) -> Option<SemanticTokensResult> {
        let uri_str = uri.as_str();
        let cached = self.cached_parses.get(uri_str)?;

        // Single pass: collect user-defined function and macro names
        let mut user_fn_names = HashSet::new();
        let mut user_macro_names = HashSet::new();
        for expr in &cached.ast {
            if let Some(items) = expr.as_list() {
                if items.len() >= 2 {
                    if let Some(head) = items[0].as_symbol() {
                        if let Some(name) = items[1].as_symbol() {
                            match head.as_str() {
                                "defun" | "defn" => {
                                    user_fn_names.insert(name);
                                }
                                "defmacro" => {
                                    user_macro_names.insert(name);
                                }
                                "define" => {
                                    // (define (f x) ...) shorthand
                                    // Already handled below
                                }
                                _ => {}
                            }
                        } else if head == "define" {
                            // (define (f args...) body) — function shorthand
                            if let Some(sig) = items[1].as_list() {
                                if !sig.is_empty() {
                                    if let Some(name) = sig[0].as_symbol() {
                                        user_fn_names.insert(name);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let lines: Vec<&str> = cached.source.lines().collect();
        // (line, UTF-16 start column, UTF-16 length, token type, modifiers)
        let mut raw_tokens: Vec<(usize, u32, usize, u32, u32)> = Vec::new();

        for (name, span) in &cached.symbol_spans {
            let (token_type, modifiers) = if sema_eval::SPECIAL_FORM_NAMES.contains(&name.as_str())
            {
                (token_types::KEYWORD, 0u32)
            } else if user_macro_names.contains(name.as_str()) {
                (token_types::MACRO, 0u32)
            } else if self.builtin_names.contains(name.as_str()) {
                (token_types::FUNCTION, token_modifiers::DEFAULT_LIBRARY)
            } else {
                // Check scope tree for classification
                match cached.scope_tree.resolve_at(name, span.line, span.col) {
                    Some(resolved) if !resolved.is_top_level => (token_types::PARAMETER, 0u32),
                    Some(_) => {
                        if user_fn_names.contains(name.as_str()) {
                            (token_types::FUNCTION, 0u32)
                        } else {
                            (token_types::VARIABLE, 0u32)
                        }
                    }
                    None => continue,
                }
            };

            // Skip multi-line tokens (shouldn't happen for symbols, but
            // the length calculation assumes a single line).
            if span.line != span.end_line || span.line == 0 {
                continue;
            }
            // Token start and length must be in UTF-16 code units (LSP spec),
            // not chars. Span columns are 1-indexed char columns; convert the
            // start, and sum the UTF-16 width of the chars in [col, end_col)
            // on the token's line.
            let char_count = span.end_col.saturating_sub(span.col);
            if char_count == 0 {
                continue;
            }
            let line_text = lines.get(span.line - 1).copied();
            let length = match line_text {
                Some(line_text) => line_text
                    .chars()
                    .skip(span.col - 1)
                    .take(char_count)
                    .map(|c| c.len_utf16())
                    .sum::<usize>(),
                None => char_count,
            };
            let start = char_col_to_utf16(line_text, span.col);
            raw_tokens.push((span.line, start, length, token_type, modifiers));
        }

        // Sort by position
        raw_tokens.sort_by_key(|&(line, col, _, _, _)| (line, col));

        // Encode as deltas
        let mut data = Vec::new();
        let mut prev_line = 0u32;
        let mut prev_start = 0u32;

        for &(line, col, length, token_type, modifiers) in &raw_tokens {
            let lsp_line = (line - 1) as u32;
            let lsp_col = col;

            // Use saturating_sub to guard against underflow from unexpected
            // out-of-order spans (shouldn't happen after sort, but defensive).
            let delta_line = lsp_line.saturating_sub(prev_line);
            let delta_start = if delta_line == 0 {
                lsp_col.saturating_sub(prev_start)
            } else {
                lsp_col
            };

            data.push(SemanticToken {
                delta_line,
                delta_start,
                length: length as u32,
                token_type,
                token_modifiers_bitset: modifiers,
            });

            prev_line = lsp_line;
            prev_start = lsp_col;
        }

        Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        }))
    }
}
