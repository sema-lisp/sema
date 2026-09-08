//! Scanning a parsed Sema AST for user definitions and imports.
//!
//! A `(module name (export ...) body...)` wrapper is transparent — its body
//! evaluates exactly like top-level code — so every scan here first
//! flattens module bodies via [`flatten_module_forms`] before matching on
//! head symbols. `scope.rs` independently walks the same forms to build the
//! lexical scope tree, but needs no such special case: its default
//! recursive case already walks into any unmatched head, `module` included.

use tower_lsp::lsp_types::*;

use sema_core::{Span, SpanMap};

use crate::helpers::{expr_span, find_name_span, span_to_range};

/// Heads that bind a name reachable elsewhere in the program — the set
/// goto-definition/hover/completion/rename/call-hierarchy treat as a "user
/// definition". Deliberately excludes `defworkflow`: unlike every head
/// here, `(defworkflow name ...)` does not bind `name` to anything — it's a
/// macro that expands to `(workflow/run (symbol->string 'name) ...)`, which
/// just runs the workflow immediately, using `name` only as a display label
/// (see the `defworkflow` macro in `sema-eval/src/prelude.rs`). Nothing can
/// reference "name" as a callee, so it has no definition to jump to and no
/// references to rename. It still gets an outline entry — see
/// [`SYMBOL_HEADS`].
pub(crate) const DEFINITION_HEADS: &[&str] = &[
    "define",
    "def",
    "defun",
    "defn",
    "defmacro",
    "define-syntax",
    "define-values",
    "define-record-type",
    "defmulti",
    "defagent",
    "deftool",
    "defpolicy",
];

/// [`DEFINITION_HEADS`] plus `defworkflow`, for document/workspace symbols
/// (the outline), which shows every named top-level form as a structural
/// marker — not just the subset that binds a reachable name.
pub(crate) const SYMBOL_HEADS: &[&str] = &[
    "define",
    "def",
    "defun",
    "defn",
    "defmacro",
    "define-syntax",
    "define-values",
    "define-record-type",
    "defmulti",
    "defagent",
    "deftool",
    "defworkflow",
    "defpolicy",
];

/// Flatten top-level containers whose bodies execute in the surrounding scope.
pub(crate) fn flatten_module_forms(forms: &[sema_core::Value]) -> Vec<&sema_core::Value> {
    let mut out = Vec::new();
    for expr in forms {
        if let Some(items) = expr.as_list() {
            if let Some(head) = items.first().and_then(sema_core::Value::as_symbol) {
                if items.len() >= 2 && head == "module" {
                    out.extend(flatten_module_forms(&items[2..]));
                    continue;
                }
                if head == "begin" || head == "progn" {
                    out.extend(flatten_module_forms(&items[1..]));
                    continue;
                }
            }
        }
        out.push(expr);
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportSpec {
    pub(crate) path: String,
    /// Start of the import form, used to model source-order visibility.
    pub(crate) position: Position,
    /// `None` imports every exported name; `Some` is a selective import.
    pub(crate) selected: Option<std::collections::HashSet<String>>,
    pub(crate) is_load: bool,
}

pub(crate) fn import_specs_from_ast(
    ast: &[sema_core::Value],
    span_map: &SpanMap,
) -> Vec<ImportSpec> {
    flatten_module_forms(ast)
        .into_iter()
        .filter_map(|expr| {
            let items = expr.as_list()?;
            let head = items.first()?.as_symbol()?;
            if head != "import" && head != "load" {
                return None;
            }
            let path = items.get(1)?.as_str()?.to_string();
            let span = expr_span(expr, span_map)?;
            let selected: std::collections::HashSet<String> = items[2..]
                .iter()
                .filter_map(sema_core::Value::as_symbol)
                .collect();
            Some(ImportSpec {
                path,
                position: Position::new(
                    span.line.saturating_sub(1) as u32,
                    span.col.saturating_sub(1) as u32,
                ),
                selected: (!selected.is_empty()).then_some(selected),
                is_load: head == "load",
            })
        })
        .collect()
}

pub(crate) fn module_exports(
    ast: &[sema_core::Value],
) -> Option<std::collections::HashSet<String>> {
    ast.iter().find_map(|expr| {
        let module = expr.as_list()?;
        (module.first()?.as_symbol()?.as_str() == "module").then_some(())?;
        module.iter().skip(2).find_map(|form| {
            let export = form.as_list()?;
            (export.first()?.as_symbol()?.as_str() == "export").then(|| {
                export[1..]
                    .iter()
                    .filter_map(sema_core::Value::as_symbol)
                    .collect()
            })
        })
    })
}

pub(crate) fn definition_is_visible(
    name: &str,
    exports: Option<&std::collections::HashSet<String>>,
    import: &ImportSpec,
) -> bool {
    if !import.is_load && exports.is_some_and(|exports| !exports.contains(name)) {
        return false;
    }
    import
        .selected
        .as_ref()
        .is_none_or(|selected| selected.contains(name))
}

/// One match found while scanning for definitions: a form whose head is in
/// the caller's `heads` list and that names a symbol via `(head name ...)`
/// or, for `define`/`def`, the function shorthand `(define (name args...)
/// body...)`.
pub(crate) struct DefMatch<'a> {
    pub(crate) head: String,
    pub(crate) name: String,
    /// The whole defining form (e.g. the `(defun ...)` list) — callers that
    /// need more than name/range (parameters, docstring, body) re-derive it
    /// from here via `.as_list()`.
    pub(crate) expr: &'a sema_core::Value,
    /// The whole form's range. `None` when the form's span isn't in
    /// `span_map` (shouldn't happen for a normally-read form, but the
    /// reader's error-recovery mode can produce ones without spans).
    pub(crate) form_range: Option<Range>,
    /// The name symbol's own range, when found precisely in `symbol_spans`
    /// (e.g. `None` when a caller passes `symbol_spans: &[]` because it only
    /// wants names, not positions). Callers that want "the best range
    /// available" should use `name_range.or(form_range)`.
    pub(crate) name_range: Option<Range>,
    /// True when `name` came from the `(define (name args...) body...)`
    /// function shorthand rather than a plain `(head name ...)` binding —
    /// only possible for `head` `"define"`/`"def"`. Callers that render a
    /// kind per head (document symbols) need this to tell a shorthand
    /// function definition apart from a plain variable one.
    pub(crate) is_shorthand: bool,
}

/// Collect names bound by a values-formal. The grammar is shared with lambda
/// formals: a symbol, a dotted list, a vector, or a map destructuring pattern.
fn collect_formal_names(value: &sema_core::Value, out: &mut Vec<String>) {
    if let Some(name) = value.as_symbol() {
        if name != "." {
            out.push(name);
        }
    } else if let Some(items) = value.as_list().or_else(|| value.as_vector()) {
        for item in items {
            collect_formal_names(item, out);
        }
    } else if let Some(map) = value.as_map_ref() {
        let keys = sema_core::Value::keyword("keys");
        for (key, pattern) in map.iter() {
            if *key == keys {
                if let Some(names) = pattern.as_list().or_else(|| pattern.as_vector()) {
                    for name in names {
                        collect_formal_names(name, out);
                    }
                }
            } else {
                collect_formal_names(pattern, out);
            }
        }
    }
}

fn push_definition<'a>(
    matches: &mut Vec<DefMatch<'a>>,
    head: &str,
    name: String,
    expr: &'a sema_core::Value,
    location: (Option<&Span>, Option<Range>),
    symbol_spans: &[(String, Span)],
    lines: &[&str],
) {
    let (form_span, form_range) = location;
    let name_range = form_span.and_then(|span| find_name_span(&name, span, symbol_spans, lines));
    matches.push(DefMatch {
        head: head.to_string(),
        name,
        expr,
        form_range,
        name_range,
        is_shorthand: false,
    });
}

impl<'a> DefMatch<'a> {
    /// The form's body — its executable/nested forms, skipping the
    /// head/name/param-list "header". Used by call-hierarchy's call-site
    /// scan so a parameter with the same name as a function elsewhere in
    /// the workspace never looks like a call to it. Precise only for the
    /// heads with a simple `(head name (params) body...)` or `(define (name
    /// args...) body...)` grammar (`defun`/`defn`/`defmacro`, `define`/
    /// `def`) — the other `SYMBOL_HEADS` (`defagent`/`deftool`/
    /// `defworkflow`/`defpolicy`) have fixed positional arguments rather
    /// than an open body sequence, so this conservatively includes
    /// everything after the name for those.
    pub(crate) fn body(&self) -> &'a [sema_core::Value] {
        let Some(items) = self.expr.as_list() else {
            return &[];
        };
        let start = match self.head.as_str() {
            "defun" | "defn" | "defmacro" => 3,
            _ => 2,
        };
        items.get(start..).unwrap_or(&[])
    }

    pub(crate) fn docstring(&self) -> Option<String> {
        let body = self.body();
        if body.len() < 2 {
            return None;
        }
        let doc = body.first()?.as_str()?.trim();
        (!doc.is_empty()).then(|| doc.to_string())
    }
}

/// Walk `forms` (already module-flattened via [`flatten_module_forms`]),
/// collecting a [`DefMatch`] for every form whose head is in `heads`. The
/// `(define (name args...) body...)` function shorthand is recognized only
/// for `define`/`def` — the only heads whose grammar allows a list in name
/// position; every other head's own grammar requires a plain symbol there.
pub(crate) fn scan_definitions<'a>(
    forms: impl IntoIterator<Item = &'a sema_core::Value>,
    heads: &[&str],
    span_map: &SpanMap,
    symbol_spans: &[(String, Span)],
    lines: &[&str],
) -> Vec<DefMatch<'a>> {
    let mut matches = Vec::new();
    for expr in forms {
        let Some(items) = expr.as_list() else {
            continue;
        };
        if items.len() < 2 {
            continue;
        }
        let Some(head) = items[0].as_symbol() else {
            continue;
        };
        if !heads.contains(&head.as_str()) {
            continue;
        }
        let form_span = expr_span(expr, span_map);
        let form_range = form_span.map(|s| span_to_range(s, lines));

        match head.as_str() {
            "define" | "def"
                if items.get(1).is_some_and(|value| {
                    value.as_vector().is_some() || value.as_map_ref().is_some()
                }) =>
            {
                let mut names = Vec::new();
                collect_formal_names(&items[1], &mut names);
                for name in names {
                    push_definition(
                        &mut matches,
                        &head,
                        name,
                        expr,
                        (form_span, form_range),
                        symbol_spans,
                        lines,
                    );
                }
                continue;
            }
            "define-values" => {
                if let Some(formals) = items.get(1) {
                    let mut names = Vec::new();
                    collect_formal_names(formals, &mut names);
                    for name in names {
                        push_definition(
                            &mut matches,
                            &head,
                            name,
                            expr,
                            (form_span, form_range),
                            symbol_spans,
                            lines,
                        );
                    }
                }
                continue;
            }
            "define-record-type" => {
                let mut names = Vec::new();
                if let Some(ctor) = items.get(2).and_then(sema_core::Value::as_list) {
                    if let Some(name) = ctor.first().and_then(sema_core::Value::as_symbol) {
                        names.push(name);
                    }
                }
                if let Some(name) = items.get(3).and_then(sema_core::Value::as_symbol) {
                    names.push(name);
                }
                for field in items.iter().skip(4) {
                    if let Some(name) = field
                        .as_list()
                        .and_then(|spec| spec.get(1))
                        .and_then(sema_core::Value::as_symbol)
                    {
                        names.push(name);
                    }
                }
                let mut candidates: Vec<&(String, Span)> = symbol_spans
                    .iter()
                    .filter(|(_, span)| form_span.is_some_and(|form| form.contains(span)))
                    .collect();
                candidates.sort_by_key(|(_, span)| (span.line, span.col));
                let mut next = 2usize;
                for name in names {
                    let found = candidates
                        .iter()
                        .enumerate()
                        .skip(next)
                        .find(|(_, (candidate, _))| candidate.as_str() == name);
                    let name_range = found.map(|(index, (_, span))| {
                        next = index + 1;
                        span_to_range(span, lines)
                    });
                    matches.push(DefMatch {
                        head: head.clone(),
                        name,
                        expr,
                        form_range,
                        name_range,
                        is_shorthand: false,
                    });
                }
                continue;
            }
            _ => {}
        }

        if let Some(name) = items[1].as_symbol() {
            // (head name ...)
            let name_range =
                form_span.and_then(|fs| find_name_span(&name, fs, symbol_spans, lines));
            matches.push(DefMatch {
                head,
                name,
                expr,
                form_range,
                name_range,
                is_shorthand: false,
            });
        } else if head == "define" || head == "def" {
            // (define (name args...) body...) — function shorthand.
            if let Some(sig) = items[1].as_list() {
                if let Some(name) = sig.first().and_then(|v| v.as_symbol()) {
                    let sig_span = expr_span(&items[1], span_map);
                    let name_range =
                        sig_span.and_then(|ss| find_name_span(&name, ss, symbol_spans, lines));
                    matches.push(DefMatch {
                        head,
                        name,
                        expr,
                        form_range,
                        name_range,
                        is_shorthand: true,
                    });
                }
            }
        }
    }
    matches
}

/// Collect user-defined names with their spans from a pre-parsed AST.
/// Returns (name, range) for each top-level form that creates a reusable binding.
/// When `symbol_spans` is provided, returns the precise span of just the name symbol;
/// otherwise falls back to the span of the entire definition form.
pub fn user_definitions_from_ast(
    ast: &[sema_core::Value],
    span_map: &SpanMap,
    symbol_spans: &[(String, Span)],
    lines: &[&str],
) -> Vec<(String, Option<Range>)> {
    scan_definitions(
        flatten_module_forms(ast),
        DEFINITION_HEADS,
        span_map,
        symbol_spans,
        lines,
    )
    .into_iter()
    .map(|m| (m.name, m.name_range.or(m.form_range)))
    .collect()
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

/// Collect reusable names from top-level definition forms.
pub fn user_definitions(text: &str) -> Vec<String> {
    user_definitions_with_spans(text)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// Extract parameter list string from a pre-parsed AST for hover display.
pub fn extract_params_from_ast(ast: &[sema_core::Value], name: &str) -> Option<String> {
    extract_params_from_forms(flatten_module_forms(ast), name)
}

/// `extract_params_from_ast`, operating on an already-flattened form list —
/// lets a caller that needs the flattened list for another purpose too
/// (e.g. `document_symbols_from_ast`, once per definition it finds) reuse it
/// instead of re-flattening the whole file on every call.
pub(crate) fn extract_params_from_forms<'a>(
    forms: impl IntoIterator<Item = &'a sema_core::Value>,
    name: &str,
) -> Option<String> {
    for expr in forms {
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
                        "define" | "def" => {
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
    for expr in flatten_module_forms(ast) {
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
            "define" | "def" => {
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
            "define" | "def" => 2,
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

/// Check if the cursor is on a string argument of an import/load form.
/// Uses the SpanMap to verify the form's span covers the cursor line.
pub fn import_path_from_ast(
    ast: &[sema_core::Value],
    span_map: &SpanMap,
    line: u32,
) -> Option<String> {
    for expr in flatten_module_forms(ast) {
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
pub fn import_path_at_cursor(text: &str, line: u32, character: u32) -> Option<String> {
    let (ast, span_map) = sema_reader::read_many_with_spans(text).ok()?;
    let source_line = text.lines().nth(line as usize)?;
    let char_col = crate::helpers::utf16_to_char_col(source_line, character as usize);
    let sema_line = line as usize + 1;
    let path = sema_reader::lexer::tokenize(text)
        .ok()?
        .iter()
        .find_map(|token| match &token.token {
            sema_reader::lexer::Token::String(candidate)
                if token.span.contains_pos(sema_line, char_col) =>
            {
                Some(candidate.clone())
            }
            _ => None,
        })?;

    // Several imports may share a line. Match the form containing this exact
    // string token instead of taking the first import whose span covers it.
    flatten_module_forms(&ast).into_iter().find_map(|expr| {
        let items = expr.as_list()?;
        let head = items.first()?.as_symbol()?;
        let candidate = items.get(1)?.as_str()?;
        (matches!(head.as_str(), "import" | "load")
            && candidate == path
            && expr_span(expr, &span_map)
                .is_some_and(|span| span.contains_pos(sema_line, char_col)))
        .then_some(path.clone())
    })
}

/// Extract all import/load path strings from a pre-parsed AST.
pub fn import_paths_from_ast(ast: &[sema_core::Value]) -> Vec<String> {
    let mut paths = Vec::new();
    for expr in flatten_module_forms(ast) {
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

/// Build `DocumentSymbol` entries from a pre-parsed AST.
#[allow(deprecated)]
pub fn document_symbols_from_ast(
    ast: &[sema_core::Value],
    span_map: &SpanMap,
    symbol_spans: &[(String, Span)],
    lines: &[&str],
) -> Vec<DocumentSymbol> {
    scan_definitions(
        flatten_module_forms(ast),
        SYMBOL_HEADS,
        span_map,
        symbol_spans,
        lines,
    )
    .into_iter()
    .map(|m| {
        let kind = symbol_kind_for(&m.head, m.is_shorthand);
        let form_range = m.form_range.unwrap_or_default();
        let selection_range = m.name_range.unwrap_or(form_range);
        // Derived from this match's own form, not a second by-name scan of
        // the file — a by-name scan would pick the wrong match's params
        // whenever two definitions share a name.
        let detail = if kind == SymbolKind::FUNCTION {
            params_of(&m)
        } else {
            None
        };
        DocumentSymbol {
            name: m.name,
            detail,
            kind,
            tags: None,
            deprecated: None,
            range: form_range,
            selection_range,
            children: None,
        }
    })
    .collect()
}

/// The outline `SymbolKind` for a definition-form head. `is_shorthand`
/// (see [`DefMatch`]) distinguishes a plain `(define x val)` (`VARIABLE`)
/// from the `(define (f args) body)` function shorthand (`FUNCTION`) —
/// only `define`/`def` can produce either shape.
pub(crate) fn symbol_kind_for(head: &str, is_shorthand: bool) -> SymbolKind {
    match head {
        "defun" | "defn" | "defworkflow" | "defmulti" | "define-record-type" => {
            SymbolKind::FUNCTION
        }
        "defmacro" | "define-syntax" => SymbolKind::OPERATOR,
        "defagent" => SymbolKind::CLASS,
        "deftool" => SymbolKind::METHOD,
        "defpolicy" => SymbolKind::VARIABLE,
        _ if is_shorthand => SymbolKind::FUNCTION, // "define"/"def" shorthand
        _ => SymbolKind::VARIABLE,                 // plain "define"/"def"
    }
}

/// The parameter-list string for a `DefMatch` already known to be
/// function-shaped (`symbol_kind_for` returned `FUNCTION`), derived from its
/// own form — never from a second scan of the file by name (see
/// `document_symbols_from_ast`). Only `defun`/`defn` and the `define`/`def`
/// shorthand actually have a positional param list at this point:
/// `defworkflow` also maps to `SymbolKind::FUNCTION` but its second slot is
/// a doc string, not params (`(defworkflow name doc meta . body)` — see
/// `DEFINITION_HEADS`' doc comment), so it deliberately falls to `None`.
pub(crate) fn params_of(m: &DefMatch) -> Option<String> {
    let items = m.expr.as_list()?;
    match m.head.as_str() {
        "defun" | "defn" => Some(sema_core::pretty_print(items.get(2)?, 80)),
        "define" | "def" if m.is_shorthand => {
            let sig = items.get(1)?.as_list()?;
            let params: Vec<_> = sig[1..]
                .iter()
                .map(|v| sema_core::pretty_print(v, 80))
                .collect();
            Some(format!("({})", params.join(" ")))
        }
        _ => None,
    }
}
