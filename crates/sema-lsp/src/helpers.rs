use std::path::{Path, PathBuf};
use std::rc::Rc;

use tower_lsp::lsp_types::*;

pub(crate) use sema_core::SemaError;
use sema_core::{Span, SpanMap};

// ── Public helpers (also used by tests) ──────────────────────────

/// Check if a character is valid inside a Sema symbol.
/// Matches the reader's `is_symbol_char` in `lexer.rs`.
pub(crate) fn is_sema_symbol_char(ch: char) -> bool {
    ch.is_alphanumeric()
        || matches!(
            ch,
            '+' | '-'
                | '*'
                | '/'
                | '!'
                | '?'
                | '<'
                | '>'
                | '='
                | '_'
                | '&'
                | '%'
                | '^'
                | '~'
                | '.'
                | '#'
        )
}

/// Convert a 0-indexed LSP UTF-16 character offset to a byte offset in a UTF-8 string.
/// Returns the byte offset, clamped to the string length.
pub fn utf16_to_byte_offset(line: &str, utf16_offset: u32) -> usize {
    let mut utf16_count = 0u32;
    for (byte_idx, ch) in line.char_indices() {
        if utf16_count >= utf16_offset {
            return byte_idx;
        }
        utf16_count += ch.len_utf16() as u32;
    }
    line.len()
}

/// Convert a byte offset in a UTF-8 string to a 0-indexed LSP UTF-16 `character`
/// offset. LSP `Position.character` counts UTF-16 code units, not bytes; this is
/// the conversion used when source-scanning machinery yields byte offsets that
/// must be reported back to the editor. A byte offset past the end (or not on a
/// char boundary) counts whole chars up to it.
pub fn byte_offset_to_utf16(line: &str, byte_offset: usize) -> u32 {
    let mut units = 0u32;
    for (byte_idx, ch) in line.char_indices() {
        if byte_idx >= byte_offset {
            return units;
        }
        units += ch.len_utf16() as u32;
    }
    units
}

/// Look up the LSP Range for an expression via its Rc pointer in the SpanMap.
pub(crate) fn expr_range(
    expr: &sema_core::Value,
    span_map: &SpanMap,
    lines: &[&str],
) -> Option<Range> {
    let rc = expr.as_list_rc()?;
    let ptr = Rc::as_ptr(&rc) as usize;
    span_map.get(&ptr).map(|s| span_to_range(s, lines))
}

/// Look up the raw Span for an expression via its Rc pointer in the SpanMap.
pub(crate) fn expr_span<'a>(expr: &sema_core::Value, span_map: &'a SpanMap) -> Option<&'a Span> {
    let rc = expr.as_list_rc()?;
    let ptr = Rc::as_ptr(&rc) as usize;
    span_map.get(&ptr)
}

/// The text of line `idx` (0-based) for an LSP position. Unlike `str::lines()`, this
/// addresses the trailing empty line produced by a final `\n` (where the cursor sits
/// when typing a new top-level form) and strips a trailing `\r`.
pub(crate) fn line_at(text: &str, idx: usize) -> Option<&str> {
    text.split('\n')
        .nth(idx)
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
}

/// Check if `inner` span is fully contained within `outer` span.
pub(crate) fn span_contains(outer: &Span, inner: &Span) -> bool {
    outer.contains(inner)
}

/// Drop symbol occurrences that sit inside quoted (data) regions — a `(quote …)` form,
/// or the non-unquoted parts of a `(quasiquote …)`. Quoted symbols are DATA, not code
/// references: including them makes rename/references/highlight rewrite quoted literals
/// (`(rename foo bar)` would silently rewrite `'(foo)` and change the program's meaning).
pub(crate) fn filter_quoted_symbol_spans(
    ast: &[sema_core::Value],
    span_map: &SpanMap,
    symbol_spans: Vec<(String, Span)>,
) -> Vec<(String, Span)> {
    let mut excl: Vec<Span> = Vec::new(); // quoted regions (data)
    let mut reincl: Vec<Span> = Vec::new(); // `,x` / `,@x` holes inside quasiquote (code)
    for e in ast {
        collect_quote_regions(e, span_map, &symbol_spans, false, &mut excl, &mut reincl);
    }
    if excl.is_empty() {
        return symbol_spans;
    }
    symbol_spans
        .into_iter()
        .filter(|(_, s)| {
            let quoted = excl.iter().any(|q| span_contains(q, s))
                && !reincl.iter().any(|u| span_contains(u, s));
            !quoted
        })
        .collect()
}

/// The span covering a quoted/unquoted ARGUMENT. The `'`/`,` head span only covers the
/// glyph, so the data is the argument's own extent: a list's `expr_span`, or a bare
/// symbol's token — which begins exactly where the head glyph ends.
fn quoted_arg_span(
    arg: &sema_core::Value,
    span_map: &SpanMap,
    symbol_spans: &[(String, Span)],
    head_span: &Span,
) -> Option<Span> {
    if let Some(s) = expr_span(arg, span_map) {
        return Some(*s);
    }
    if arg.as_symbol().is_some() {
        return symbol_spans
            .iter()
            .find(|(_, sp)| sp.line == head_span.end_line && sp.col == head_span.end_col)
            .map(|(_, sp)| *sp);
    }
    None
}

/// Recursively mark quoted regions (and the unquoted holes inside quasiquote) for
/// [`filter_quoted_symbol_spans`]. A plain `(quote …)` is wholly data; `(quasiquote …)`
/// is data except inside `(unquote …)` / `(unquote-splicing …)`, which is code again.
fn collect_quote_regions(
    expr: &sema_core::Value,
    span_map: &SpanMap,
    symbol_spans: &[(String, Span)],
    in_quote: bool,
    excl: &mut Vec<Span>,
    reincl: &mut Vec<Span>,
) {
    let Some(items) = expr.as_list() else {
        return;
    };
    let head = items.first().and_then(|v| v.as_symbol());
    let form_span = expr_span(expr, span_map).copied();
    match head.as_deref() {
        Some("quote") if !in_quote => {
            if let Some(fs) = form_span {
                for arg in items.iter().skip(1) {
                    if let Some(s) = quoted_arg_span(arg, span_map, symbol_spans, &fs) {
                        excl.push(s);
                    }
                }
            }
            // plain quote has no unquote semantics — the argument is wholly data.
        }
        Some("quasiquote") if !in_quote => {
            if let Some(fs) = form_span {
                for arg in items.iter().skip(1) {
                    if let Some(s) = quoted_arg_span(arg, span_map, symbol_spans, &fs) {
                        excl.push(s);
                    }
                }
            }
            for item in items.iter() {
                collect_quote_regions(item, span_map, symbol_spans, true, excl, reincl);
            }
        }
        Some("unquote") | Some("unquote-splicing") if in_quote => {
            if let Some(fs) = form_span {
                for arg in items.iter().skip(1) {
                    if let Some(s) = quoted_arg_span(arg, span_map, symbol_spans, &fs) {
                        reincl.push(s);
                    }
                }
            }
            for item in items.iter().skip(1) {
                collect_quote_regions(item, span_map, symbol_spans, false, excl, reincl);
            }
        }
        _ => {
            for item in items.iter() {
                collect_quote_regions(item, span_map, symbol_spans, in_quote, excl, reincl);
            }
        }
    }
}

/// Find the precise span of a symbol `name` within a form's span.
/// Searches `symbol_spans` for the first occurrence of `name` contained in `form_span`.
pub(crate) fn find_name_span(
    name: &str,
    form_span: &Span,
    symbol_spans: &[(String, Span)],
    lines: &[&str],
) -> Option<Range> {
    symbol_spans
        .iter()
        .find(|(sym_name, sym_span)| sym_name == name && span_contains(form_span, sym_span))
        .map(|(_, span)| span_to_range(span, lines))
}

/// Walk backwards from `pos` to find the byte offset where a symbol starts.
pub(crate) fn symbol_start(line: &str, pos: usize) -> usize {
    line[..pos]
        .char_indices()
        .rev()
        .take_while(|&(_, ch)| is_sema_symbol_char(ch))
        .last()
        .map(|(i, _)| i)
        .unwrap_or(pos)
}

/// Extract span from a SemaError, unwrapping wrapper layers.
pub fn error_span(err: &SemaError) -> Option<&Span> {
    match err.inner() {
        SemaError::Reader { span, .. } => Some(span),
        _ => None,
    }
}

/// Convert a 1-indexed Sema char column to a 0-indexed LSP UTF-16 `character`.
///
/// Sema spans count **characters**; LSP `Position.character` counts **UTF-16
/// code units**. They differ only on lines containing non-BMP characters
/// (emoji, rare CJK-ext), where one char is two UTF-16 units. `line` is the
/// source text of the span's line; when it is `None` we fall back to the raw
/// char index (correct for BMP-only lines).
pub fn char_col_to_utf16(line: Option<&str>, col_1indexed: usize) -> u32 {
    let char_idx = col_1indexed.saturating_sub(1);
    match line {
        Some(l) => l.chars().take(char_idx).map(|c| c.len_utf16() as u32).sum(),
        None => char_idx as u32,
    }
}

/// Convert a 0-indexed LSP UTF-16 `character` offset to a 1-indexed Sema char
/// column (the inverse of [`char_col_to_utf16`]). Used when mapping an incoming
/// editor `Position` to the column the scope/lexer machinery expects.
pub fn utf16_to_char_col(line: &str, utf16_offset: usize) -> usize {
    let mut units = 0usize;
    for (i, ch) in line.chars().enumerate() {
        if units >= utf16_offset {
            return i + 1;
        }
        units += ch.len_utf16();
    }
    line.chars().count() + 1
}

/// Convert a 1-indexed Sema `Span` to a 0-indexed LSP `Range`, mapping columns
/// from char indices to UTF-16 code units using `lines` (the document's lines).
///
/// FOOTGUN: passing `&[]` (or lines that don't contain the span's line) silently
/// falls back to emitting char-index columns instead of UTF-16 code units. That
/// is only correct for BMP-only text — any astral-plane char (emoji, etc.) on or
/// before the span on its line will yield wrong LSP positions. Only pass `&[]`
/// when the returned `Range` is immediately discarded (e.g. callers that use just
/// the symbol names). For any range that reaches the client, pass the real lines
/// (e.g. `cached.source.lines().collect()` or `text.lines().collect()`).
pub fn span_to_range(span: &Span, lines: &[&str]) -> Range {
    let start_line = span.line.saturating_sub(1);
    let end_line = span.end_line.saturating_sub(1);
    Range {
        start: Position {
            line: start_line as u32,
            character: char_col_to_utf16(lines.get(start_line).copied(), span.col),
        },
        end: Position {
            line: end_line as u32,
            character: char_col_to_utf16(lines.get(end_line).copied(), span.end_col),
        },
    }
}

/// Build a diagnostic message from a `SemaError`, appending hint/note if present.
pub(crate) fn format_error_message(err: &SemaError) -> String {
    let mut message = match err.inner() {
        SemaError::Reader { message, .. } => message.clone(),
        other => other.to_string(),
    };
    if let Some(hint) = err.hint() {
        message.push_str(&format!("\nhint: {hint}"));
    }
    if let Some(note) = err.note() {
        message.push_str(&format!("\nnote: {note}"));
    }
    message
}

/// Convert a SemaError into a diagnostic with the given severity.
pub(crate) fn error_diagnostic(
    err: &SemaError,
    severity: DiagnosticSeverity,
    lines: &[&str],
) -> Diagnostic {
    let range = error_span(err)
        .map(|s| span_to_range(s, lines))
        .unwrap_or_default();
    Diagnostic {
        range,
        severity: Some(severity),
        source: Some("sema".to_string()),
        message: format_error_message(err),
        ..Default::default()
    }
}

/// Parse source text and return diagnostics.
pub fn parse_diagnostics(text: &str) -> Vec<Diagnostic> {
    let lines: Vec<&str> = text.lines().collect();
    match sema_reader::read_many_with_spans(text) {
        Ok(_) => vec![],
        Err(err) => vec![error_diagnostic(&err, DiagnosticSeverity::ERROR, &lines)],
    }
}

/// Run the VM compilation pipeline on parsed expressions to catch
/// deeper errors (unbound variables, arity mismatches, invalid forms).
pub fn compile_diagnostics(exprs: &[sema_core::Value], lines: &[&str]) -> Vec<Diagnostic> {
    match sema_vm::compile_program(exprs, None) {
        Ok(_) => vec![],
        Err(err) => vec![error_diagnostic(&err, DiagnosticSeverity::WARNING, lines)],
    }
}

/// Parse and compile-check source text, returning all diagnostics.
/// Uses error recovery to report multiple parse errors at once.
pub fn analyze_document(text: &str) -> Vec<Diagnostic> {
    let lines: Vec<&str> = text.lines().collect();
    let (exprs, _spans, _symbol_spans, errors) = sema_reader::read_many_with_spans_recover(text);
    let mut diags: Vec<Diagnostic> = errors
        .iter()
        .map(|err| error_diagnostic(err, DiagnosticSeverity::ERROR, &lines))
        .collect();
    // Only run compile diagnostics when there are no parse errors,
    // since missing forms would cause false unbound-variable errors.
    if diags.is_empty() {
        diags.extend(compile_diagnostics(&exprs, &lines));
    }
    diags
}

/// Extract the symbol prefix at the given cursor position for completion.
/// `byte_offset` is a byte index into the UTF-8 line string.
/// Returns the prefix string (may be empty).
pub fn extract_prefix(line: &str, byte_offset: usize) -> &str {
    let end = byte_offset.min(line.len());
    &line[symbol_start(line, end)..end]
}

/// Return (index, LSP range) for each top-level list form that has a span.
/// Non-list forms (bare atoms) and forms without spans are skipped.
pub fn top_level_ranges(
    exprs: &[sema_core::Value],
    span_map: &SpanMap,
    lines: &[&str],
) -> Vec<(usize, Range)> {
    exprs
        .iter()
        .enumerate()
        .filter_map(|(i, expr)| Some((i, expr_range(expr, span_map, lines)?)))
        .collect()
}

/// Extract the full symbol at the given cursor position.
/// `byte_offset` is a byte index into the UTF-8 line string.
/// Returns the symbol string (may be empty if cursor is not on a symbol).
pub fn extract_symbol_at(line: &str, byte_offset: usize) -> &str {
    let pos = byte_offset.min(line.len());
    let start = symbol_start(line, pos);

    // Walk forwards to find end
    let end = line[pos..]
        .char_indices()
        .take_while(|&(_, ch)| is_sema_symbol_char(ch))
        .last()
        .map(|(i, ch)| pos + i + ch.len_utf8())
        .unwrap_or(pos);

    &line[start..end]
}

/// Collect user-defined names with their spans from a pre-parsed AST.
/// Returns (name, range) for each `define`/`defun`/`defn`/`defmacro`/`defagent`/`deftool`.
/// When `symbol_spans` is provided, returns the precise span of just the name symbol;
/// otherwise falls back to the span of the entire definition form.
pub fn user_definitions_from_ast(
    ast: &[sema_core::Value],
    span_map: &SpanMap,
    symbol_spans: &[(String, Span)],
    lines: &[&str],
) -> Vec<(String, Option<Range>)> {
    let mut defs = Vec::new();
    for expr in ast {
        if let Some(items) = expr.as_list() {
            if items.len() >= 2 {
                if let Some(head) = items[0].as_symbol() {
                    match head.as_str() {
                        "define" | "defun" | "defn" | "defmacro" | "defagent" | "deftool" => {
                            let form_span = expr_span(expr, span_map);
                            // (define name ...) or (defun name (...) ...)
                            if let Some(name) = items[1].as_symbol() {
                                let name_range = form_span
                                    .and_then(|fs| find_name_span(&name, fs, symbol_spans, lines));
                                let range = name_range
                                    .or_else(|| form_span.map(|s| span_to_range(s, lines)));
                                defs.push((name, range));
                            }
                            // (define (name args...) body) - function shorthand
                            else if let Some(sig) = items[1].as_list() {
                                if !sig.is_empty() {
                                    if let Some(name) = sig[0].as_symbol() {
                                        // Search within the signature list's span
                                        let sig_span = expr_span(&items[1], span_map);
                                        let name_range = sig_span.and_then(|ss| {
                                            find_name_span(&name, ss, symbol_spans, lines)
                                        });
                                        let range = name_range
                                            .or_else(|| form_span.map(|s| span_to_range(s, lines)));
                                        defs.push((name, range));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    defs
}

/// Convenience wrapper: parse text and collect user definitions with spans.
pub fn user_definitions_with_spans(text: &str) -> Vec<(String, Option<Range>)> {
    let lines: Vec<&str> = text.lines().collect();
    let (ast, span_map, symbol_spans) = match sema_reader::read_many_with_symbol_spans(text) {
        Ok(result) => result,
        Err(_) => return vec![],
    };
    user_definitions_from_ast(&ast, &span_map, &symbol_spans, &lines)
}

/// Extract parameter list string from a pre-parsed AST for hover display.
pub fn extract_params_from_ast(ast: &[sema_core::Value], name: &str) -> Option<String> {
    for expr in ast {
        if let Some(items) = expr.as_list() {
            if items.len() >= 3 {
                if let Some(head) = items[0].as_symbol() {
                    match head.as_str() {
                        "defun" | "defn" | "defmacro" | "deftool" => {
                            if let Some(sym) = items[1].as_symbol() {
                                if sym == name {
                                    return Some(sema_core::pretty_print(&items[2], 80));
                                }
                            }
                        }
                        "define" => {
                            if let Some(sig) = items[1].as_list() {
                                if !sig.is_empty() {
                                    if let Some(sym) = sig[0].as_symbol() {
                                        if sym == name {
                                            let params: Vec<_> = sig[1..]
                                                .iter()
                                                .map(|v| sema_core::pretty_print(v, 80))
                                                .collect();
                                            return Some(format!("({})", params.join(" ")));
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    None
}

/// Extract the docstring of a user-defined function `name` from the AST.
///
/// Follows the Clojure convention: a leading string literal in a function body is the docstring
/// **only when at least one more body form follows** (otherwise the string is the function's return
/// value, not documentation). `(defun f (x) "doc" body)` → `Some("doc")`; `(defun f (x) "ret")` →
/// `None`. No language change is needed — a leading string body form is already legal.
pub fn extract_docstring_from_ast(ast: &[sema_core::Value], name: &str) -> Option<String> {
    for expr in ast {
        let items = match expr.as_list() {
            Some(items) if items.len() >= 2 => items,
            _ => continue,
        };
        let head = match items[0].as_symbol() {
            Some(h) => h,
            None => continue,
        };
        // Body starts at index 2 for (defun name (params) body...) and the (define (name ...) ...)
        // shorthand. Match the function's name.
        let matches_name = match head.as_str() {
            "defun" | "defn" | "defmacro" => items[1].as_symbol().as_deref() == Some(name),
            "define" => {
                items[1]
                    .as_list()
                    .and_then(|sig| sig.first().and_then(|v| v.as_symbol()))
                    .as_deref()
                    == Some(name)
            }
            _ => false,
        };
        if !matches_name {
            continue;
        }
        // Body start: `(defun name (params) body...)` → index 3; `(define (name ..) body...)` → 2.
        let body_start = match head.as_str() {
            "define" => 2,
            _ => 3,
        };
        let body = items.get(body_start..).unwrap_or(&[]);
        if body.len() >= 2 {
            if let Some(s) = body[0].as_str() {
                let s = s.trim();
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
        return None; // found the definition; no docstring
    }
    None
}

/// Convenience wrapper: parse text and extract parameter list.
pub fn extract_params(text: &str, name: &str) -> Option<String> {
    let ast = match sema_reader::read_many_with_spans(text) {
        Ok((values, _)) => values,
        Err(_) => return None,
    };
    extract_params_from_ast(&ast, name)
}

/// Resolve an import/load path relative to a document URI.
/// Returns the absolute path if resolvable.
pub fn resolve_import_path(uri: &Url, path_str: &str) -> Option<PathBuf> {
    let file_path = uri.to_file_path().ok()?;
    let dir = file_path.parent()?;
    if Path::new(path_str).is_absolute() {
        Some(PathBuf::from(path_str))
    } else if sema_core::resolve::is_package_import(path_str) {
        sema_core::resolve::resolve_package_import(path_str).ok()
    } else {
        Some(dir.join(path_str))
    }
}

/// Check if the cursor is on a string argument of an import/load form.
/// Uses the SpanMap to verify the form's span covers the cursor line.
pub fn import_path_from_ast(
    ast: &[sema_core::Value],
    span_map: &SpanMap,
    line: u32,
) -> Option<String> {
    for expr in ast {
        if let Some(items) = expr.as_list() {
            if items.len() >= 2 {
                if let Some(head) = items[0].as_symbol() {
                    if head == "import" || head == "load" {
                        if let Some(path) = items[1].as_str() {
                            // Use SpanMap to check if this form covers the cursor line.
                            // Line numbers are encoding-independent, so compare the
                            // raw span directly (no char↔UTF-16 conversion needed).
                            let covers_line = expr_span(expr, span_map)
                                .map(|s| {
                                    line >= s.line.saturating_sub(1) as u32
                                        && line <= s.end_line.saturating_sub(1) as u32
                                })
                                .unwrap_or(false);
                            if covers_line {
                                return Some(path.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Convenience wrapper: parse text and check for import path at cursor.
pub fn import_path_at_cursor(text: &str, line: u32, _character: u32) -> Option<String> {
    let (ast, span_map) = sema_reader::read_many_with_spans(text).ok()?;
    import_path_from_ast(&ast, &span_map, line)
}

/// Extract all import/load path strings from a pre-parsed AST.
pub fn import_paths_from_ast(ast: &[sema_core::Value]) -> Vec<String> {
    let mut paths = Vec::new();
    for expr in ast {
        if let Some(items) = expr.as_list() {
            if items.len() >= 2 {
                if let Some(head) = items[0].as_symbol() {
                    if head == "import" || head == "load" {
                        if let Some(path) = items[1].as_str() {
                            paths.push(path.to_string());
                        }
                    }
                }
            }
        }
    }
    paths
}

/// Parse parameter names from a params string like "(a b)" or "(a b . rest)".
pub(crate) fn parse_param_names(params_str: &str) -> Vec<String> {
    let inner = params_str
        .trim()
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(params_str.trim());
    // Strip line comments before splitting
    let cleaned: String = inner
        .lines()
        .map(|line| line.split(';').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");
    cleaned
        .split_whitespace()
        .filter(|&s| s != ".")
        .map(|s| s.to_string())
        .collect()
}

/// Find the 0-indexed (line, col) positions of arguments in a list form
/// by scanning the source text. Skips the function name (first token) and
/// returns positions of up to `max_args` subsequent arguments.
pub(crate) fn find_arg_positions_in_form(
    form_span: &Span,
    lines: &[&str],
    max_args: usize,
) -> Vec<(usize, usize)> {
    let mut positions = Vec::new();
    let start_line = form_span.line.saturating_sub(1); // 0-indexed
    let start_col = form_span.col; // 1-indexed, points at '('
    let end_line = form_span.end_line.saturating_sub(1);
    let end_col = form_span.end_col.saturating_sub(1);

    // State machine to walk through the form text
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut in_comment = false;
    let mut token_count = 0usize; // 0 = opening paren, 1 = func name, 2+ = args
    let mut in_token = false;

    for line_idx in start_line..=end_line.min(lines.len().saturating_sub(1)) {
        let line = match lines.get(line_idx) {
            Some(l) => l,
            None => break,
        };
        let col_start = if line_idx == start_line { start_col } else { 1 };
        let col_end = if line_idx == end_line {
            end_col + 1
        } else {
            line.len()
        };

        for (byte_idx, ch) in line.char_indices() {
            let col_0 = byte_idx; // 0-indexed byte position
            if col_0 + 1 < col_start && line_idx == start_line {
                continue;
            }
            if col_0 > col_end && line_idx == end_line {
                break;
            }

            if in_comment {
                continue; // comments end at newline, handled by line iteration
            }

            if in_string {
                if escape {
                    escape = false;
                    continue;
                }
                if ch == '\\' {
                    escape = true;
                    continue;
                }
                if ch == '"' {
                    in_string = false;
                }
                continue;
            }

            match ch {
                ';' => {
                    in_comment = true;
                    in_token = false;
                }
                '"' => {
                    if depth == 1 && !in_token {
                        // String argument starts here
                        token_count += 1;
                        if token_count >= 2 && positions.len() < max_args {
                            positions.push((line_idx, col_0));
                        }
                    }
                    in_string = true;
                    in_token = true;
                }
                '(' | '[' | '{' => {
                    if depth == 1 && !in_token {
                        token_count += 1;
                        if token_count >= 2 && positions.len() < max_args {
                            positions.push((line_idx, col_0));
                        }
                    }
                    depth += 1;
                    // Opening paren of the form itself (depth 0→1) should not
                    // mark in_token, so the function name is counted as token 1.
                    if depth > 1 {
                        in_token = true;
                    }
                }
                ')' | ']' | '}' => {
                    depth -= 1;
                    if depth <= 0 {
                        return positions;
                    }
                    if depth == 1 {
                        in_token = false;
                    }
                }
                _ if ch.is_whitespace() => {
                    if depth == 1 {
                        in_token = false;
                    }
                }
                _ => {
                    if depth == 1 && !in_token {
                        // Start of a new token at depth 1
                        token_count += 1;
                        if token_count >= 2 && positions.len() < max_args {
                            positions.push((line_idx, col_0));
                        }
                        in_token = true;
                    }
                }
            }

            if positions.len() >= max_args {
                return positions;
            }
        }
        in_comment = false; // Reset at end of line
    }

    positions
}

/// Extract parameter names from a builtin doc string by parsing the first
/// code example. Looks for `(func_name arg1 arg2 ...)` in a ```sema block.
/// True if `token` looks like a parameter name rather than an argument expression.
/// Rejects compound expressions (anything with parens/brackets/braces), string literals,
/// quoted forms, numbers, and empty tokens.
fn looks_like_param_name(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    // Reject compound expressions
    if token.contains(&['(', ')', '[', ']', '{', '}'][..]) {
        return false;
    }
    // Reject string literals
    if token.starts_with('"') {
        return false;
    }
    // Reject quoted / quasiquoted / unquoted forms
    if token.starts_with('\'') || token.starts_with('`') || token.starts_with(",") {
        return false;
    }
    // Reject pure numbers (integers, floats, scientific notation)
    if token.parse::<f64>().is_ok() {
        return false;
    }
    true
}

/// Best-effort fallback: extract parameter names from the first ` ```sema ` code example
/// in a doc body. This is only used when the entry has no structured `params` — it
/// presumes the first example shows a function *signature* like `(foo a b)` rather than
/// a function *call* with complex argument expressions like `(map (fn (x) x) lst)`.
///
/// Tokens that don't look like simple parameter names (compound expressions, literals,
/// quoted forms, numbers) are discarded. If any token in the first matching call fails
/// this check, the whole call is skipped so that higher-order functions don't produce
/// nonsensical inlay hints like `(fn (x) (* x x)):`.
pub(crate) fn extract_params_from_doc(doc: &str, func_name: &str) -> Option<Vec<String>> {
    // Find the first sema code block
    let code_start = doc.find("```sema\n")?;
    let code_body = &doc[code_start + 8..];
    let code_end = code_body.find("```")?;
    let code = &code_body[..code_end];

    // Find a call of the form (func_name arg1 arg2 ...)
    let prefix = format!("({func_name} ");
    for line in code.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            // Extract tokens until closing paren, stripping nested parens
            let mut params = Vec::new();
            let mut depth = 0i32;
            let mut current = String::new();
            for ch in rest.chars() {
                match ch {
                    ')' if depth == 0 => {
                        let token = current.trim().to_string();
                        if !token.is_empty() {
                            params.push(token);
                        }
                        break;
                    }
                    '(' => {
                        depth += 1;
                        current.push(ch);
                    }
                    ')' => {
                        depth -= 1;
                        current.push(ch);
                    }
                    _ if ch.is_whitespace() && depth == 0 => {
                        let token = current.trim().to_string();
                        if !token.is_empty() {
                            params.push(token);
                        }
                        current.clear();
                    }
                    _ => current.push(ch),
                }
            }
            // Only accept this call if EVERY token looks like a parameter name.
            // This prevents `(map (fn (x) x) '(1 2 3))` from yielding nonsensical
            // hints like `(fn (x) x):` — instead we skip it and return None.
            if !params.is_empty() && params.iter().all(|p| looks_like_param_name(p)) {
                return Some(params);
            }
        }
    }
    None
}

/// Find the enclosing function call at the given cursor position.
/// Returns `(function_name, active_parameter_index)` where active_parameter_index
/// is the 0-based index of the argument the cursor is currently on.
pub fn find_enclosing_call(text: &str, line: u32, character: u32) -> Option<(String, usize)> {
    // Convert line/character to byte offset
    let mut byte_offset = 0;
    for (i, l) in text.split('\n').enumerate() {
        if i == line as usize {
            byte_offset += utf16_to_byte_offset(l, character);
            break;
        }
        byte_offset += l.len() + 1;
    }
    let cursor = byte_offset.min(text.len());

    // Ensure cursor is on a valid UTF-8 char boundary
    let prefix = text.get(..cursor)?;

    // Forward scan tracking paren positions and string/comment state
    let mut paren_stack: Vec<(usize, u8)> = Vec::new(); // (byte_pos, delimiter)
    let mut in_string = false;
    let mut escape = false;
    let mut in_comment = false;

    for (i, ch) in prefix.char_indices() {
        if in_comment {
            if ch == '\n' {
                in_comment = false;
            }
            continue;
        }
        if in_string {
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' {
                escape = true;
                continue;
            }
            if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            ';' => in_comment = true,
            '"' => {
                in_string = true;
                escape = false;
            }
            '(' => paren_stack.push((i, b'(')),
            ')' => {
                paren_stack.pop();
            }
            '[' => paren_stack.push((i, b'[')),
            ']' => {
                paren_stack.pop();
            }
            '{' => paren_stack.push((i, b'{')),
            '}' => {
                paren_stack.pop();
            }
            _ => {}
        }
    }

    // Find the innermost unclosed `(` (not `[` or `{`)
    let paren_pos = paren_stack
        .into_iter()
        .rev()
        .find(|&(_, ch)| ch == b'(')
        .map(|(pos, _)| pos)?;

    let after_paren = &text[paren_pos + 1..cursor];

    // Extract function name (first whitespace-delimited token)
    let trimmed = after_paren.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    let func_end = trimmed
        .find(|ch: char| {
            ch.is_whitespace() || matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | ';' | '"')
        })
        .unwrap_or(trimmed.len());
    let func_name = &trimmed[..func_end];
    if func_name.is_empty() {
        return None;
    }

    // Count complete depth-0 arguments after function name
    let rest = &trimmed[func_end..];
    let mut arg_count = 0usize;
    let mut nest = 0i32;
    let mut in_atom = false;
    let mut in_str = false;
    let mut esc = false;
    let mut in_cmt = false;

    for ch in rest.chars() {
        if in_cmt {
            if ch == '\n' {
                in_cmt = false;
            }
            continue;
        }
        if in_str {
            if esc {
                esc = false;
                continue;
            }
            if ch == '\\' {
                esc = true;
                continue;
            }
            if ch == '"' {
                in_str = false;
                if nest == 0 {
                    arg_count += 1;
                    in_atom = false;
                }
            }
            continue;
        }
        match ch {
            ';' => {
                in_cmt = true;
                if nest == 0 && in_atom {
                    arg_count += 1;
                    in_atom = false;
                }
            }
            '"' => {
                in_str = true;
                esc = false;
                if nest == 0 && !in_atom {
                    in_atom = true;
                }
            }
            '(' | '[' | '{' => {
                if nest == 0 && !in_atom {
                    in_atom = true;
                }
                nest += 1;
            }
            ')' | ']' | '}' => {
                if nest > 0 {
                    nest -= 1;
                    if nest == 0 && in_atom {
                        arg_count += 1;
                        in_atom = false;
                    }
                }
            }
            _ if ch.is_whitespace() => {
                if nest == 0 && in_atom {
                    arg_count += 1;
                    in_atom = false;
                }
            }
            _ => {
                if nest == 0 && !in_atom {
                    in_atom = true;
                }
            }
        }
    }

    Some((func_name.to_string(), arg_count))
}

/// Build `DocumentSymbol` entries from a pre-parsed AST.
#[allow(deprecated)]
pub fn document_symbols_from_ast(
    ast: &[sema_core::Value],
    span_map: &SpanMap,
    symbol_spans: &[(String, Span)],
    lines: &[&str],
) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    for expr in ast {
        if let Some(items) = expr.as_list() {
            if items.len() >= 2 {
                if let Some(head) = items[0].as_symbol() {
                    let (name, kind, name_range) = match head.as_str() {
                        "defun" | "defn" => {
                            if let Some(name) = items[1].as_symbol() {
                                let fs = expr_span(expr, span_map);
                                let nr =
                                    fs.and_then(|s| find_name_span(&name, s, symbol_spans, lines));
                                (name, SymbolKind::FUNCTION, nr)
                            } else {
                                continue;
                            }
                        }
                        "defmacro" => {
                            if let Some(name) = items[1].as_symbol() {
                                let fs = expr_span(expr, span_map);
                                let nr =
                                    fs.and_then(|s| find_name_span(&name, s, symbol_spans, lines));
                                (name, SymbolKind::OPERATOR, nr)
                            } else {
                                continue;
                            }
                        }
                        "defagent" => {
                            if let Some(name) = items[1].as_symbol() {
                                let fs = expr_span(expr, span_map);
                                let nr =
                                    fs.and_then(|s| find_name_span(&name, s, symbol_spans, lines));
                                (name, SymbolKind::CLASS, nr)
                            } else {
                                continue;
                            }
                        }
                        "deftool" => {
                            if let Some(name) = items[1].as_symbol() {
                                let fs = expr_span(expr, span_map);
                                let nr =
                                    fs.and_then(|s| find_name_span(&name, s, symbol_spans, lines));
                                (name, SymbolKind::METHOD, nr)
                            } else {
                                continue;
                            }
                        }
                        "define" => {
                            if let Some(name) = items[1].as_symbol() {
                                let fs = expr_span(expr, span_map);
                                let nr =
                                    fs.and_then(|s| find_name_span(&name, s, symbol_spans, lines));
                                (name, SymbolKind::VARIABLE, nr)
                            } else if let Some(sig) = items[1].as_list() {
                                if !sig.is_empty() {
                                    if let Some(name) = sig[0].as_symbol() {
                                        let ss = expr_span(&items[1], span_map);
                                        let nr = ss.and_then(|s| {
                                            find_name_span(&name, s, symbol_spans, lines)
                                        });
                                        (name, SymbolKind::FUNCTION, nr)
                                    } else {
                                        continue;
                                    }
                                } else {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        }
                        _ => continue,
                    };

                    let form_range = expr_range(expr, span_map, lines).unwrap_or_default();
                    let selection_range = name_range.unwrap_or(form_range);

                    let detail = if kind == SymbolKind::FUNCTION {
                        extract_params_from_ast(ast, &name)
                    } else {
                        None
                    };

                    symbols.push(DocumentSymbol {
                        name,
                        detail,
                        kind,
                        tags: None,
                        deprecated: None,
                        range: form_range,
                        selection_range,
                        children: None,
                    });
                }
            }
        }
    }
    symbols
}

/// Collect user-defined names from top-level `define`/`defun`/`defn`/`defmacro`/`defagent`/`deftool` forms.
pub fn user_definitions(text: &str) -> Vec<String> {
    user_definitions_with_spans(text)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}
