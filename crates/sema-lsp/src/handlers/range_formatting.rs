//! Document range formatting (`textDocument/rangeFormatting`).
//!
//! Formatting an arbitrary sub-expression in a Lisp is unsafe: a selection can cut through the
//! middle of a form, and `sema-fmt` only knows how to format complete top-level forms. So instead
//! of feeding the raw selection to the formatter, we expand the requested range to the smallest set
//! of *whole* top-level forms it overlaps, format exactly those, and return a single [`TextEdit`]
//! scoped to that span.

use tower_lsp::lsp_types::*;

use crate::handlers::formatting::formatter_options;
use crate::helpers::{top_level_ranges, utf16_to_byte_offset};
use crate::state::BackendState;

impl BackendState {
    /// Format the whole top-level forms overlapped by `range` with `sema-fmt`.
    ///
    /// Returns a single [`TextEdit`] spanning those forms, an empty edit list when they are
    /// already formatted, or `None` when the document can't be parsed, the URI is unknown, or the
    /// range overlaps no top-level form (e.g. it sits entirely in whitespace/comments).
    pub(crate) fn handle_range_formatting(
        &self,
        uri: &Url,
        range: &Range,
        options: &FormattingOptions,
    ) -> Option<Vec<TextEdit>> {
        let text = self.documents.get(uri.as_str())?;
        let lines: Vec<&str> = text.split('\n').collect();

        // Map the whole document to its top-level forms (in source order).
        let (ast, span_map) = sema_reader::read_many_with_spans(text).ok()?;
        let forms = top_level_ranges(&ast, &span_map, &lines);

        // Keep the forms whose span overlaps the requested range. Two ranges overlap unless one
        // ends before the other starts (touching at a boundary does NOT count as overlap, so a
        // cursor resting just after a form's `)` doesn't drag the next form in).
        let overlapping: Vec<&Range> = forms
            .iter()
            .map(|(_, r)| r)
            .filter(|r| !range_before(r, range) && !range_before(range, r))
            .collect();

        // Nothing overlaps (range is in whitespace/comments between or outside forms).
        let first = overlapping.first()?;
        let last = overlapping.last()?;

        // Expand to the contiguous span covering every overlapped whole form.
        let span = Range {
            start: first.start,
            end: last.end,
        };

        let start_byte = position_to_byte(&lines, &span.start);
        let end_byte = position_to_byte(&lines, &span.end);
        let slice = text.get(start_byte..end_byte)?;

        let fmt_opts = formatter_options(options);
        let formatted = sema_fmt::format_source(slice, &fmt_opts).ok()?;

        // `format_source` always emits a trailing newline; preserve the original slice's trailing
        // (non-)newline so we don't inject one mid-document or drop the document's final newline.
        let formatted = match (slice.ends_with('\n'), formatted.strip_suffix('\n')) {
            (false, Some(stripped)) => stripped.to_string(),
            _ => formatted,
        };

        if formatted == slice {
            return Some(vec![]);
        }
        Some(vec![TextEdit {
            range: span,
            new_text: formatted,
        }])
    }
}

/// True when `a` ends strictly before `b` starts (no overlap, not even touching).
fn range_before(a: &Range, b: &Range) -> bool {
    (a.end.line, a.end.character) <= (b.start.line, b.start.character)
}

/// Convert an LSP [`Position`] (UTF-16 columns) to a byte offset into the full document.
fn position_to_byte(lines: &[&str], pos: &Position) -> usize {
    let line = pos.line as usize;
    // Byte offset of the start of `pos.line` (each line contributes its bytes + the '\n' we split on).
    let mut byte = lines.iter().take(line).map(|l| l.len() + 1).sum::<usize>();
    if let Some(l) = lines.get(line) {
        byte += utf16_to_byte_offset(l, pos.character);
    }
    byte
}
