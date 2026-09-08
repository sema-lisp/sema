//! Document formatting (`textDocument/formatting`).

use tower_lsp::lsp_types::*;

use crate::state::BackendState;

pub(crate) fn formatter_options(options: &FormattingOptions) -> sema_fmt::FormatOptions {
    sema_fmt::FormatOptions {
        indent: (options.tab_size as usize).clamp(1, sema_fmt::FormatOptions::MAX_INDENT),
        use_tabs: !options.insert_spaces,
        ..sema_fmt::FormatOptions::default()
    }
}

impl BackendState {
    /// Format the whole document with `sema-fmt`. Returns a single full-document edit, an empty
    /// edit list when already formatted, or `None` (no change) when the source can't be parsed.
    pub(crate) fn handle_formatting(
        &self,
        uri: &Url,
        options: &FormattingOptions,
    ) -> Option<Vec<TextEdit>> {
        let text = self.documents.get(uri.as_str())?;
        // sema-fmt defaults (width 80, align off), except the editor's configured
        // indent size maps onto the formatter's indent width.
        let fmt_opts = formatter_options(options);
        let formatted = match sema_fmt::format_source(text, &fmt_opts) {
            Ok(f) => f,
            // Don't disturb the buffer when the source has syntax errors.
            Err(_) => return None,
        };
        if formatted == *text {
            return Some(vec![]);
        }
        // Replace the entire document. Compute the exact end position (UTF-16) so the edit
        // covers every existing character including a trailing newline.
        let mut end_line = 0u32;
        let mut end_char = 0u32;
        for (i, line) in text.split('\n').enumerate() {
            end_line = i as u32;
            end_char = line.chars().map(|c| c.len_utf16() as u32).sum();
        }
        Some(vec![TextEdit {
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: end_line,
                    character: end_char,
                },
            },
            new_text: formatted,
        }])
    }
}
