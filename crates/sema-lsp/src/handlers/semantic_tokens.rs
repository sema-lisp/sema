//! Semantic tokens (`textDocument/semanticTokens/full`).

use std::collections::HashSet;

use tower_lsp::lsp_types::*;

use crate::definitions::*;
use crate::helpers::char_col_to_utf16;
use crate::state::{token_modifiers, token_types, BackendState};

/// Heads this handler classifies into a highlight-color bucket. A narrower
/// set than [`SYMBOL_HEADS`]/[`DEFINITION_HEADS`] — `defagent`/`deftool`/
/// `defpolicy` names get no special token type here, so they fall through
/// to the ordinary scope-tree-based classification below.
pub(crate) const NAME_CLASS_HEADS: &[&str] =
    &["defun", "defn", "defmacro", "defworkflow", "define", "def"];

impl BackendState {
    pub(crate) fn handle_semantic_tokens_full(&self, uri: &Url) -> Option<SemanticTokensResult> {
        let uri_str = uri.as_str();
        let cached = self.cached_parses.get(uri_str)?;

        // Single pass: collect user-defined function and macro names. Ranges
        // are discarded (symbol_spans/lines: &[]) — only the head/name matter.
        let mut user_fn_names = HashSet::new();
        let mut user_macro_names = HashSet::new();
        let mut workflow_names = HashSet::new();
        for m in scan_definitions(
            flatten_module_forms(&cached.ast),
            NAME_CLASS_HEADS,
            &cached.span_map,
            &[],
            &[],
        ) {
            match m.head.as_str() {
                "defun" | "defn" => {
                    user_fn_names.insert(m.name);
                }
                "defmacro" => {
                    user_macro_names.insert(m.name);
                }
                "defworkflow" => {
                    workflow_names.insert(m.name);
                }
                // "define" | "def": only the (define (f args) body) shorthand
                // counts as a function name (see `DefMatch::is_shorthand`).
                _ if m.is_shorthand => {
                    user_fn_names.insert(m.name);
                }
                _ => {}
            }
        }

        let lines: Vec<&str> = cached.source.lines().collect();
        // (line, UTF-16 start column, UTF-16 length, token type, modifiers)
        let mut raw_tokens: Vec<(usize, u32, usize, u32, u32)> = Vec::new();

        for (name, span) in &cached.symbol_spans {
            let (token_type, modifiers) = if sema_eval::SPECIAL_FORM_NAMES.contains(&name.as_str())
                || SYMBOL_HEADS.contains(&name.as_str())
            {
                (token_types::KEYWORD, 0u32)
            } else if user_macro_names.contains(name.as_str()) {
                (token_types::MACRO, 0u32)
            } else if workflow_names.contains(name.as_str()) {
                (token_types::FUNCTION, 0u32)
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
