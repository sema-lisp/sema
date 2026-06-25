use std::collections::HashMap;

use tower_lsp::lsp_types::*;

use sema_core::{Caps, Sandbox, Span};

use crate::helpers::*;
use crate::server::normalize_lsp_message_body;
use crate::state::{default_sema_binary, position_in_range, BackendState, CachedParse};
use crate::{builtin_docs, scope};

// ── doc coverage gate ────────────────────────────────────────

/// Every registered builtin and special form must have a doc entry in the sema-docs index
/// (internal `__vm-*` helpers excluded). Always-on gate — if this fails, document the listed
/// names in `crates/sema-docs/stdlib/` (or `special-forms/`) and run `make docs`.
#[test]
fn builtin_doc_coverage() {
    use std::collections::HashSet;
    let sandbox = Sandbox::deny(Caps::ALL);
    let interp = sema_eval::Interpreter::new_with_sandbox(&sandbox);
    let mut names: HashSet<String> = HashSet::new();
    interp.global_env.iter_bindings(|spur, _| {
        names.insert(sema_core::resolve(spur));
    });
    for sf in sema_eval::SPECIAL_FORM_NAMES {
        names.insert(sf.to_string());
    }
    let docs = builtin_docs::BuiltinDocs::load();
    let mut missing: Vec<String> = names
        .into_iter()
        // Exclude internal VM helper forms (`__vm-*`, `__*`) — not user-facing.
        .filter(|n| !n.starts_with("__"))
        // `llm/io-sleep-once` is a throwaway AwaitIo spike leaf (proves async
        // I/O overlap; see docs/plans/2026-06-23-async-agent-parallelization.md
        // §5). It is not a user-facing builtin and is intentionally undocumented.
        .filter(|n| n != "llm/io-sleep-once")
        .filter(|n| !docs.contains(n))
        .collect();
    missing.sort();
    assert!(
        missing.is_empty(),
        "{} builtins/special-forms lack docs:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

// ── formatting ───────────────────────────────────────────────

fn format_state(uri: &str, source: &str) -> (BackendState, Url) {
    let mut docs = HashMap::new();
    docs.insert(uri.to_string(), source.to_string());
    let state = BackendState::new_without_builtins(docs, "sema".to_string());
    (state, Url::parse(uri).unwrap())
}

#[test]
fn formatting_unformatted_doc_returns_full_document_edit() {
    let (state, uri) = format_state("file:///fmt.sema", "(define   x    42)");
    let opts = FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        ..Default::default()
    };
    let edits = state
        .handle_formatting(&uri, &opts)
        .expect("should produce edits");
    assert_eq!(edits.len(), 1);
    // The edit must match what the formatter produces and start at the document origin.
    let expected =
        sema_fmt::format_source("(define   x    42)", &sema_fmt::FormatOptions::default()).unwrap();
    assert_eq!(edits[0].new_text, expected);
    assert_eq!(
        edits[0].range.start,
        Position {
            line: 0,
            character: 0
        }
    );
}

#[test]
fn formatting_already_formatted_doc_returns_no_edits() {
    let formatted =
        sema_fmt::format_source("(define x 42)", &sema_fmt::FormatOptions::default()).unwrap();
    let (state, uri) = format_state("file:///clean.sema", &formatted);
    let opts = FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        ..Default::default()
    };
    let edits = state
        .handle_formatting(&uri, &opts)
        .expect("should return Some(empty)");
    assert!(edits.is_empty());
}

#[test]
fn formatting_unparseable_doc_returns_none() {
    let (state, uri) = format_state("file:///bad.sema", "(define x");
    let opts = FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        ..Default::default()
    };
    assert!(state.handle_formatting(&uri, &opts).is_none());
}

#[test]
fn formatting_unknown_uri_returns_none() {
    let (state, _) = format_state("file:///known.sema", "(define x 1)");
    let missing = Url::parse("file:///missing.sema").unwrap();
    let opts = FormattingOptions::default();
    assert!(state.handle_formatting(&missing, &opts).is_none());
}

// ── range formatting ─────────────────────────────────────────

fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Range {
    Range {
        start: Position {
            line: sl,
            character: sc,
        },
        end: Position {
            line: el,
            character: ec,
        },
    }
}

fn range_fmt_opts() -> FormattingOptions {
    FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        ..Default::default()
    }
}

#[test]
fn range_formatting_expands_to_overlapped_whole_form() {
    // Two top-level forms; selection touches only the first (a sub-slice of it).
    let src = "(define   x    42)\n(define y 1)\n";
    let (state, uri) = format_state("file:///rf.sema", src);
    // Range covers part of the first form's interior (chars 1..10 on line 0).
    let edits = state
        .handle_range_formatting(&uri, &range(0, 1, 0, 10), &range_fmt_opts())
        .expect("should produce edits");
    assert_eq!(edits.len(), 1);
    let edit = &edits[0];
    // The edit is scoped to the whole first form only (line 0), not the second.
    assert_eq!(
        edit.range.start,
        Position {
            line: 0,
            character: 0
        }
    );
    assert_eq!(
        edit.range.end,
        Position {
            line: 0,
            character: 18
        }
    );
    // The reformatted slice is the canonical form, with no trailing newline added
    // (the original slice had none — it ended at the form's `)`).
    assert_eq!(edit.new_text, "(define x 42)");
}

#[test]
fn range_formatting_spans_multiple_overlapped_forms() {
    let src = "(define   x 1)\n(define    y 2)\n(define z 3)\n";
    let (state, uri) = format_state("file:///rf2.sema", src);
    // Selection straddles the first two forms (ends on line 1).
    let edits = state
        .handle_range_formatting(&uri, &range(0, 3, 1, 5), &range_fmt_opts())
        .expect("edits");
    assert_eq!(edits.len(), 1);
    let edit = &edits[0];
    assert_eq!(
        edit.range.start,
        Position {
            line: 0,
            character: 0
        }
    );
    // Covers through the end of the second form (line 1), not the third.
    assert_eq!(edit.range.end.line, 1);
    assert_eq!(edit.new_text, "(define x 1)\n(define y 2)");
}

#[test]
fn range_formatting_already_formatted_returns_no_edits() {
    let src = "(define x 1)\n(define y 2)\n";
    let (state, uri) = format_state("file:///rf3.sema", src);
    let edits = state
        .handle_range_formatting(&uri, &range(0, 0, 0, 12), &range_fmt_opts())
        .expect("Some(empty)");
    assert!(edits.is_empty());
}

#[test]
fn range_formatting_unparseable_returns_none() {
    let (state, uri) = format_state("file:///rfbad.sema", "(define x");
    assert!(state
        .handle_range_formatting(&uri, &range(0, 0, 0, 5), &range_fmt_opts())
        .is_none());
}

#[test]
fn range_formatting_unknown_uri_returns_none() {
    let (state, _) = format_state("file:///rfknown.sema", "(define x 1)");
    let missing = Url::parse("file:///rfmissing.sema").unwrap();
    assert!(state
        .handle_range_formatting(&missing, &range(0, 0, 0, 5), &range_fmt_opts())
        .is_none());
}

#[test]
fn range_formatting_in_blank_gap_overlaps_nothing() {
    // Selection sits entirely on the blank line between two forms → no whole form
    // overlaps → None (don't touch the buffer).
    let src = "(define x 1)\n\n(define y 2)\n";
    let (state, uri) = format_state("file:///rfgap.sema", src);
    assert!(state
        .handle_range_formatting(&uri, &range(1, 0, 1, 0), &range_fmt_opts())
        .is_none());
}

// ── selection range ──────────────────────────────────────────

/// Build a state with a populated parse cache for `source`, mirroring the dispatch path.
fn parsed_state(uri: &str, source: &str) -> (BackendState, Url) {
    let mut state = BackendState::new_without_builtins(HashMap::new(), "sema".to_string());
    let (ast, span_map, symbol_spans) = sema_reader::read_many_with_symbol_spans(source).unwrap();
    // Mirror the production build path (server.rs / state.rs): drop quoted symbols.
    let symbol_spans = crate::helpers::filter_quoted_symbol_spans(&ast, &span_map, symbol_spans);
    let scope_tree = scope::ScopeTree::build(&ast, &span_map, &symbol_spans);
    state.cached_parses.insert(
        uri.to_string(),
        CachedParse {
            ast,
            span_map,
            symbol_spans,
            scope_tree,
            source: source.to_string(),
        },
    );
    state.documents.insert(uri.to_string(), source.to_string());
    (state, Url::parse(uri).unwrap())
}

/// Innermost → outermost chain of ranges for a SelectionRange.
fn selection_chain(sr: &SelectionRange) -> Vec<Range> {
    let mut out = vec![sr.range];
    let mut parent = sr.parent.as_ref();
    while let Some(node) = parent {
        out.push(node.range);
        parent = node.parent.as_ref();
    }
    out
}

#[test]
fn selection_range_expands_from_symbol_outward() {
    //                       0         1
    //                       0123456789012345678
    let src = "(define x (+ a b))";
    let (state, uri) = parsed_state("file:///sel.sema", src);
    // Cursor on the inner `a` (character 13).
    let pos = Position {
        line: 0,
        character: 13,
    };
    let result = state.handle_selection_range(&uri, &[pos]).unwrap();
    assert_eq!(result.len(), 1);

    let chain = selection_chain(&result[0]);
    assert!(
        chain.len() >= 2,
        "expected symbol + enclosing forms, got {chain:?}"
    );

    // Innermost covers the cursor; outermost is the whole top-level form starting at col 0.
    assert!(position_in_range(&pos, &chain[0]));
    let outer = chain.last().unwrap();
    assert_eq!(
        outer.start,
        Position {
            line: 0,
            character: 0
        }
    );
    assert_eq!(
        outer.end,
        Position {
            line: 0,
            character: 18
        }
    );

    // Each parent strictly contains its child (monotonic growth).
    for pair in chain.windows(2) {
        let (child, parent) = (&pair[0], &pair[1]);
        assert!(position_in_range(&child.start, parent));
        assert!(position_in_range(&child.end, parent));
        assert_ne!(child, parent);
    }
}

#[test]
fn selection_range_unknown_uri_returns_none() {
    let (state, _) = parsed_state("file:///known.sema", "(define x 1)");
    let missing = Url::parse("file:///missing.sema").unwrap();
    assert!(state
        .handle_selection_range(
            &missing,
            &[Position {
                line: 0,
                character: 1
            }]
        )
        .is_none());
}

// ── document links ───────────────────────────────────────────

#[test]
fn document_links_resolves_import_path() {
    //                       0         1         2
    //                       012345678901234567890
    let src = "(import \"./foo.sema\")";
    let (state, uri) = parsed_state("file:///proj/main.sema", src);
    let links = state.handle_document_links(&uri).expect("links");
    assert_eq!(links.len(), 1);
    let link = &links[0];
    // Range covers just the path literal (chars 9..19), not the whole form.
    assert_eq!(link.range.start.character, 9);
    assert_eq!(link.range.end.character, 19);
    assert!(link.target.as_ref().unwrap().path().ends_with("foo.sema"));
}

#[test]
fn document_links_empty_when_no_imports() {
    let (state, uri) = parsed_state("file:///proj/main.sema", "(define x 1)");
    assert!(state.handle_document_links(&uri).unwrap().is_empty());
}

// ── call hierarchy ───────────────────────────────────────────

const CALL_SRC: &str = "(defun helper (x) (* x x))\n(defun main () (helper 5))";

#[test]
fn call_hierarchy_prepare_resolves_definition() {
    let (state, uri) = parsed_state("file:///ch.sema", CALL_SRC);
    // Cursor on "helper" in its definition (line 0, char 8).
    let items = state
        .handle_call_hierarchy_prepare(
            &uri,
            &Position {
                line: 0,
                character: 8,
            },
        )
        .expect("prepare");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "helper");
}

#[test]
fn call_hierarchy_incoming_finds_callers() {
    let (state, uri) = parsed_state("file:///ch.sema", CALL_SRC);
    let item = state
        .handle_call_hierarchy_prepare(
            &uri,
            &Position {
                line: 0,
                character: 8,
            },
        )
        .unwrap()
        .remove(0);
    let incoming = state
        .handle_call_hierarchy_incoming(&item)
        .expect("incoming");
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].from.name, "main");
    assert_eq!(incoming[0].from_ranges.len(), 1);
}

#[test]
fn call_hierarchy_outgoing_finds_callees() {
    let (state, uri) = parsed_state("file:///ch.sema", CALL_SRC);
    // Prepare on "main" (line 1, char 8).
    let item = state
        .handle_call_hierarchy_prepare(
            &uri,
            &Position {
                line: 1,
                character: 8,
            },
        )
        .unwrap()
        .remove(0);
    assert_eq!(item.name, "main");
    let outgoing = state
        .handle_call_hierarchy_outgoing(&item)
        .expect("outgoing");
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].to.name, "helper");
}

// ── completion resolve ───────────────────────────────────────

#[test]
fn completion_resolve_enriches_user_definition() {
    let (state, uri) = parsed_state("file:///c.sema", "(defun greet (name greeting) name)");
    // A bare user-def completion item as produced by handle_complete (data = uri).
    let item = CompletionItem {
        label: "greet".to_string(),
        kind: Some(CompletionItemKind::FUNCTION),
        data: Some(serde_json::Value::String(uri.as_str().to_string())),
        ..Default::default()
    };
    let resolved = state.handle_completion_resolve(item);
    let doc = match resolved.documentation {
        Some(Documentation::MarkupContent(c)) => c.value,
        _ => panic!("expected markdown documentation"),
    };
    assert!(doc.contains("(greet name greeting)"), "got: {doc}");
}

#[test]
fn completion_special_form_carries_syntax_detail() {
    // Special forms with a manual `syntax` label (sema-docs frontmatter) must surface it
    // as the completion item's inline `detail`, same as builtins do via `signature()`.
    let mut state = BackendState::new_without_builtins(HashMap::new(), "sema".to_string());
    state.builtin_docs = builtin_docs::BuiltinDocs::load();
    let uri = Url::parse("file:///sf.sema").unwrap();
    state
        .documents
        .insert(uri.as_str().to_string(), "(le".to_string());
    let items = state.handle_complete(
        &uri,
        &Position {
            line: 0,
            character: 3,
        },
    );
    let item = items
        .iter()
        .find(|i| i.label == "let")
        .expect("`let` completion item");
    let entry = state.builtin_docs.get("let").expect("docs for `let`");
    assert!(entry.syntax.is_some(), "`let` should carry a syntax label");
    assert_eq!(
        item.detail.as_deref(),
        Some(builtin_docs::signature(entry).as_str())
    );
}

#[test]
fn completion_resolve_keeps_existing_documentation() {
    let state = BackendState::new_without_builtins(HashMap::new(), "sema".to_string());
    let item = CompletionItem {
        label: "already".to_string(),
        documentation: Some(Documentation::String("kept".to_string())),
        ..Default::default()
    };
    let resolved = state.handle_completion_resolve(item);
    match resolved.documentation {
        Some(Documentation::String(s)) => assert_eq!(s, "kept"),
        _ => panic!("documentation should be preserved untouched"),
    }
}

// ── default sema binary ──────────────────────────────────────

#[test]
fn default_sema_binary_prefers_current_exe() {
    // In a test run, current_exe() is the test binary — so the default must
    // resolve to that concrete path, never the bare "sema" PATH fallback.
    let default = default_sema_binary();
    let expected = std::env::current_exe().unwrap();
    assert_eq!(default, expected.to_str().unwrap());
    assert_ne!(default, "sema");
}

// ── span_to_range ────────────────────────────────────────────

#[test]
fn span_to_range_converts_1_indexed_to_0() {
    let span = Span::new(1, 1, 1, 5);
    let range = span_to_range(&span, &[]);
    assert_eq!(range.start.line, 0);
    assert_eq!(range.start.character, 0);
    assert_eq!(range.end.line, 0);
    assert_eq!(range.end.character, 4);
}

#[test]
fn shutdown_request_with_null_params_is_normalized() {
    let body = br#"{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}"#;

    let normalized = normalize_lsp_message_body(body);
    let value: serde_json::Value = serde_json::from_slice(&normalized).unwrap();

    assert_eq!(value["method"], "shutdown");
    assert!(value.get("params").is_none());
}

#[test]
fn span_to_range_multiline() {
    let span = Span::new(3, 10, 5, 2);
    let range = span_to_range(&span, &[]);
    assert_eq!(range.start.line, 2);
    assert_eq!(range.start.character, 9);
    assert_eq!(range.end.line, 4);
    assert_eq!(range.end.character, 1);
}

// LSP-1: Sema spans count chars; LSP Position.character counts UTF-16 code
// units. On a line with an astral char (🎉 = 2 UTF-16 units) the two diverge,
// and an unconverted range points at the wrong column → rename corrupts source.
#[test]
fn span_to_range_maps_astral_line_to_utf16() {
    // "(x 🎉 y)" — chars: ( x SPACE 🎉 SPACE y ) ; `y` is char col 6..7.
    let lines = ["(x 🎉 y)"];
    let span = Span::new(1, 6, 1, 7);

    // Char-index fallback (no line context) is wrong by the emoji's extra unit.
    assert_eq!(span_to_range(&span, &[]).start.character, 5);

    // With the line, the emoji counts as 2 UTF-16 units → correct columns.
    let r = span_to_range(&span, &lines);
    assert_eq!(r.start.character, 6, "y should start at UTF-16 unit 6");
    assert_eq!(r.end.character, 7);
}

#[test]
fn char_utf16_conversions_roundtrip_through_astral() {
    let line = "(x 🎉 y)";
    // Editor → Sema: UTF-16 character 6 is `y`, which is char col 6 (1-indexed).
    assert_eq!(utf16_to_char_col(line, 6), 6);
    // Sema → editor: char col 6 maps back to 6 UTF-16 units.
    assert_eq!(char_col_to_utf16(Some(line), 6), 6);
    // Pure ASCII is an identity mapping in both directions.
    assert_eq!(utf16_to_char_col("abc", 2), 3);
    assert_eq!(char_col_to_utf16(Some("abc"), 3), 2);
}

// ── parse_diagnostics ────────────────────────────────────────

#[test]
fn valid_code_no_diagnostics() {
    let diags = parse_diagnostics("(define x 42)");
    assert!(diags.is_empty());
}

#[test]
fn empty_input_no_diagnostics() {
    let diags = parse_diagnostics("");
    assert!(diags.is_empty());
}

#[test]
fn unclosed_paren_produces_diagnostic() {
    let diags = parse_diagnostics("(define x");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    assert_eq!(diags[0].source.as_deref(), Some("sema"));
}

#[test]
fn unterminated_string_produces_diagnostic() {
    let diags = parse_diagnostics("(define x \"hello)");
    assert_eq!(diags.len(), 1);
}

#[test]
fn diagnostic_includes_hint() {
    // quote requires an expression after it
    let diags = parse_diagnostics("'");
    assert_eq!(diags.len(), 1);
    // The reader error for bare quote includes a hint
    assert!(
        diags[0].message.contains("hint:") || diags[0].message.contains("quote"),
        "message was: {}",
        diags[0].message
    );
}

// ── extract_prefix ───────────────────────────────────────────

#[test]
fn prefix_basic_symbol() {
    // cursor after 'x': (define x| 42)
    assert_eq!(extract_prefix("(define x 42)", 9), "x");
}

#[test]
fn prefix_namespaced() {
    assert_eq!(extract_prefix("(string/trim s)", 12), "string/trim");
}

#[test]
fn prefix_at_start_of_line() {
    assert_eq!(extract_prefix("define", 3), "def");
}

#[test]
fn prefix_after_open_paren() {
    assert_eq!(extract_prefix("(def", 4), "def");
}

#[test]
fn prefix_empty_after_space() {
    assert_eq!(extract_prefix("(define ", 8), "");
}

#[test]
fn prefix_predicate() {
    assert_eq!(extract_prefix("(string? x)", 8), "string?");
}

#[test]
fn prefix_bang() {
    assert_eq!(extract_prefix("(set! x 1)", 5), "set!");
}

// ── user_definitions ─────────────────────────────────────────

#[test]
fn user_defs_defun() {
    let defs = user_definitions("(defun foo (x) x)");
    assert_eq!(defs, vec!["foo"]);
}

#[test]
fn user_defs_defn() {
    let defs = user_definitions("(defn bar (x y) (+ x y))");
    assert_eq!(defs, vec!["bar"]);
}

#[test]
fn user_defs_define() {
    let defs = user_definitions("(define pi 3.14)");
    assert_eq!(defs, vec!["pi"]);
}

#[test]
fn user_defs_define_function_shorthand() {
    let defs = user_definitions("(define (square x) (* x x))");
    assert_eq!(defs, vec!["square"]);
}

#[test]
fn user_defs_multiple() {
    let src = "(define x 1)\n(defun f (a) a)\n(defmacro m (x) x)";
    let defs = user_definitions(src);
    assert_eq!(defs, vec!["x", "f", "m"]);
}

#[test]
fn user_defs_bad_syntax_returns_empty() {
    let defs = user_definitions("(define x");
    assert!(defs.is_empty());
}

// ── compile_diagnostics / analyze_document ─────────────────

#[test]
fn compile_invalid_define_produces_warning() {
    // (define) has wrong arity — caught by the VM lowering pass
    let diags = analyze_document("(define)");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
    assert_eq!(diags[0].source.as_deref(), Some("sema"));
    assert!(
        diags[0].message.contains("define"),
        "expected mention of 'define', got: {}",
        diags[0].message
    );
}

#[test]
fn compile_valid_code_no_diagnostics() {
    let diags = analyze_document("(define x 1) (+ x 1)");
    assert!(diags.is_empty(), "expected no diagnostics, got: {diags:?}");
}

#[test]
fn compile_parse_error_returns_error_not_warning() {
    let diags = analyze_document("(define x");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
}

#[test]
fn compile_empty_input_no_diagnostics() {
    let diags = analyze_document("");
    assert!(diags.is_empty());
}

#[test]
fn compile_empty_lambda_body_produces_warning() {
    let diags = analyze_document("(lambda (x))");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
}

// ── error recovery (multiple diagnostics) ────────────────────

#[test]
fn multiple_stray_closers_reports_multiple_errors() {
    let diags = analyze_document(") (define x 1) )");
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
        .collect();
    assert_eq!(errors.len(), 2, "expected 2 errors, got: {errors:?}");
}

#[test]
fn error_recovery_still_reports_single_error() {
    // Single unclosed paren still produces exactly one error
    let diags = analyze_document("(define x");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
}

// ── extract_symbol_at ────────────────────────────────────────

#[test]
fn symbol_at_cursor_middle() {
    assert_eq!(extract_symbol_at("(define foo 42)", 9), "foo");
}

#[test]
fn symbol_at_cursor_start() {
    assert_eq!(extract_symbol_at("define", 0), "define");
}

#[test]
fn symbol_at_cursor_namespaced() {
    assert_eq!(extract_symbol_at("(string/trim s)", 5), "string/trim");
}

#[test]
fn symbol_at_cursor_end_of_symbol() {
    assert_eq!(extract_symbol_at("(define foo 42)", 10), "foo");
}

#[test]
fn symbol_at_cursor_on_paren() {
    assert_eq!(extract_symbol_at("(define foo)", 0), "");
}

#[test]
fn symbol_at_cursor_predicate() {
    assert_eq!(extract_symbol_at("(null? x)", 3), "null?");
}

#[test]
fn symbol_at_cursor_operator_plus() {
    assert_eq!(extract_symbol_at("(+ 1 2)", 1), "+");
}

#[test]
fn symbol_at_cursor_operator_lte() {
    assert_eq!(extract_symbol_at("(<= x 5)", 1), "<=");
}

#[test]
fn symbol_at_cursor_operator_arrow() {
    assert_eq!(extract_symbol_at("(-> x f g)", 1), "->");
}

#[test]
fn prefix_operator_plus() {
    assert_eq!(extract_prefix("(+ 1 2)", 2), "+");
}

#[test]
fn prefix_operator_lte() {
    assert_eq!(extract_prefix("(<= x 5)", 3), "<=");
}

// ── utf16_to_byte_offset ─────────────────────────────────────

#[test]
fn utf16_ascii_identity() {
    assert_eq!(utf16_to_byte_offset("hello", 3), 3);
}

#[test]
fn utf16_past_end() {
    assert_eq!(utf16_to_byte_offset("hi", 10), 2);
}

#[test]
fn utf16_with_multibyte() {
    // "aé" — 'é' is 2 bytes in UTF-8, 1 code unit in UTF-16
    let s = "aéb";
    assert_eq!(utf16_to_byte_offset(s, 0), 0); // 'a'
    assert_eq!(utf16_to_byte_offset(s, 1), 1); // 'é' starts at byte 1
    assert_eq!(utf16_to_byte_offset(s, 2), 3); // 'b' starts at byte 3
}

#[test]
fn utf16_with_emoji() {
    // "a🌍b" — '🌍' is 4 bytes in UTF-8, 2 code units in UTF-16
    let s = "a🌍b";
    assert_eq!(utf16_to_byte_offset(s, 0), 0); // 'a'
    assert_eq!(utf16_to_byte_offset(s, 1), 1); // '🌍' starts at byte 1
    assert_eq!(utf16_to_byte_offset(s, 3), 5); // 'b' starts at byte 5
}

#[test]
fn symbol_at_after_multibyte_comment() {
    // "; é\n(define x 1)" — symbol 'define' after multibyte char
    let line = "(define x 1)";
    assert_eq!(extract_symbol_at(line, 1), "define");
}

// ── span helpers ─────────────────────────────────────────────

#[test]
fn span_contains_basic() {
    let outer = sema_core::Span::new(1, 1, 3, 10);
    let inner = sema_core::Span::new(2, 5, 2, 8);
    assert!(span_contains(&outer, &inner));
}

#[test]
fn span_contains_same() {
    let span = sema_core::Span::new(1, 1, 1, 10);
    assert!(span_contains(&span, &span));
}

#[test]
fn span_contains_outside() {
    let outer = sema_core::Span::new(1, 1, 1, 10);
    let inner = sema_core::Span::new(2, 1, 2, 5);
    assert!(!span_contains(&outer, &inner));
}

#[test]
fn find_name_span_basic() {
    let sym_spans = vec![
        ("define".to_string(), sema_core::Span::new(1, 2, 1, 8)),
        ("foo".to_string(), sema_core::Span::new(1, 9, 1, 12)),
    ];
    let form_span = sema_core::Span::new(1, 1, 1, 20);
    let result = find_name_span("foo", &form_span, &sym_spans, &[]);
    assert!(result.is_some());
    let range = result.unwrap();
    assert_eq!(range.start.line, 0); // LSP is 0-indexed
    assert_eq!(range.start.character, 8); // col 9 → character 8
}

#[test]
fn find_name_span_not_in_form() {
    let sym_spans = vec![("foo".to_string(), sema_core::Span::new(5, 1, 5, 4))];
    let form_span = sema_core::Span::new(1, 1, 1, 20);
    assert!(find_name_span("foo", &form_span, &sym_spans, &[]).is_none());
}

// ── precise name spans ─────────────────────────────────────

#[test]
fn precise_name_span_defun() {
    // "(defun foo (x) x)" — "foo" is at col 8-11 (1-indexed)
    let defs = user_definitions_with_spans("(defun foo (x) x)");
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].0, "foo");
    let range = defs[0].1.unwrap();
    // LSP is 0-indexed, so col 8 → character 7
    assert_eq!(range.start.line, 0);
    assert_eq!(range.start.character, 7);
    // Span should cover just "foo" (3 chars), not the whole form
    assert_eq!(range.end.character - range.start.character, 3);
}

#[test]
fn precise_name_span_define() {
    // "(define x 42)" — "x" is at col 9 (1-indexed)
    let defs = user_definitions_with_spans("(define x 42)");
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].0, "x");
    let range = defs[0].1.unwrap();
    assert_eq!(range.start.character, 8); // col 9 → character 8
    assert_eq!(range.end.character - range.start.character, 1); // "x" = 1 char
}

#[test]
fn precise_name_span_define_function_shorthand() {
    // "(define (square x) (* x x))" — "square" is inside the signature list
    let defs = user_definitions_with_spans("(define (square x) (* x x))");
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].0, "square");
    let range = defs[0].1.unwrap();
    // "square" starts at col 10 (1-indexed) → character 9
    assert_eq!(range.start.character, 9);
    assert_eq!(range.end.character - range.start.character, 6); // "square" = 6 chars
}

// ── user_definitions_with_spans ──────────────────────────────

#[test]
fn user_defs_with_spans_basic() {
    let defs = user_definitions_with_spans("(defun foo (x) x)");
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].0, "foo");
    assert!(defs[0].1.is_some(), "should have a span");
}

#[test]
fn user_defs_with_spans_multiple() {
    let src = "(define x 1)\n(defun f (a) a)";
    let defs = user_definitions_with_spans(src);
    assert_eq!(defs.len(), 2);
    assert_eq!(defs[0].0, "x");
    assert_eq!(defs[1].0, "f");
    // Both should have spans
    assert!(defs[0].1.is_some());
    assert!(defs[1].1.is_some());
    // Second def should be on a later line
    let r0 = defs[0].1.unwrap();
    let r1 = defs[1].1.unwrap();
    assert!(r1.start.line > r0.start.line);
}

#[test]
fn user_defs_with_spans_bad_syntax() {
    let defs = user_definitions_with_spans("(define x");
    assert!(defs.is_empty());
}

// ── extract_params ───────────────────────────────────────────

#[test]
fn extract_params_defun() {
    let params = extract_params("(defun add (a b) (+ a b))", "add");
    assert!(params.is_some());
    assert!(params.unwrap().contains("a"));
}

#[test]
fn extract_params_define_shorthand() {
    let params = extract_params("(define (square x) (* x x))", "square");
    assert!(params.is_some());
    assert!(params.unwrap().contains("x"));
}

#[test]
fn extract_params_not_found() {
    let params = extract_params("(defun foo (x) x)", "bar");
    assert!(params.is_none());
}

#[test]
fn extract_params_define_variable() {
    // (define x 42) is not a function — no params
    let params = extract_params("(define x 42)", "x");
    assert!(params.is_none());
}

// ── import_path_at_cursor ────────────────────────────────────

#[test]
fn import_path_on_import_line() {
    let src = "(import \"utils.sema\")";
    let path = import_path_at_cursor(src, 0, 10);
    assert_eq!(path, Some("utils.sema".to_string()));
}

#[test]
fn import_path_on_load_line() {
    let src = "(load \"config.sema\")";
    let path = import_path_at_cursor(src, 0, 10);
    assert_eq!(path, Some("config.sema".to_string()));
}

#[test]
fn import_path_wrong_line() {
    let src = "(define x 1)\n(import \"utils.sema\")";
    // cursor on first line, not the import
    let path = import_path_at_cursor(src, 0, 5);
    assert!(path.is_none());
}

#[test]
fn import_path_on_correct_line_multiline() {
    let src = "(define x 1)\n(import \"utils.sema\")";
    let path = import_path_at_cursor(src, 1, 10);
    assert_eq!(path, Some("utils.sema".to_string()));
}

// ── resolve_import_path ──────────────────────────────────────

#[test]
fn resolve_relative_path() {
    let uri = Url::parse("file:///project/src/main.sema").unwrap();
    let resolved = resolve_import_path(&uri, "utils.sema");
    assert_eq!(
        resolved,
        Some(std::path::PathBuf::from("/project/src/utils.sema"))
    );
}

#[test]
fn resolve_absolute_path() {
    let uri = Url::parse("file:///project/src/main.sema").unwrap();
    let resolved = resolve_import_path(&uri, "/lib/utils.sema");
    assert_eq!(resolved, Some(std::path::PathBuf::from("/lib/utils.sema")));
}

// ── import_paths_from_ast ────────────────────────────────────

#[test]
fn import_paths_extracts_imports() {
    let src = "(import \"utils.sema\")\n(import \"lib.sema\")\n(define x 1)";
    let (ast, _) = sema_reader::read_many_with_spans(src).unwrap();
    let paths = import_paths_from_ast(&ast);
    assert_eq!(paths, vec!["utils.sema", "lib.sema"]);
}

#[test]
fn import_paths_extracts_loads() {
    let src = "(load \"config.sema\")\n(define x 1)";
    let (ast, _) = sema_reader::read_many_with_spans(src).unwrap();
    let paths = import_paths_from_ast(&ast);
    assert_eq!(paths, vec!["config.sema"]);
}

#[test]
fn import_paths_empty_when_no_imports() {
    let src = "(define x 1)\n(defun f (a) a)";
    let (ast, _) = sema_reader::read_many_with_spans(src).unwrap();
    let paths = import_paths_from_ast(&ast);
    assert!(paths.is_empty());
}

#[test]
fn import_paths_mixed() {
    let src = "(import \"a.sema\")\n(load \"b.sema\")\n(import \"c.sema\" (foo bar))";
    let (ast, _) = sema_reader::read_many_with_spans(src).unwrap();
    let paths = import_paths_from_ast(&ast);
    assert_eq!(paths, vec!["a.sema", "b.sema", "c.sema"]);
}

// ── find_enclosing_call ──────────────────────────────────────

#[test]
fn enclosing_call_simple() {
    // (foo |) — cursor after space, 0 args
    let result = find_enclosing_call("(foo )", 0, 5);
    assert_eq!(result, Some(("foo".to_string(), 0)));
}

#[test]
fn enclosing_call_one_arg() {
    // (foo bar |) — cursor after bar, 1 complete arg
    let result = find_enclosing_call("(foo bar )", 0, 9);
    assert_eq!(result, Some(("foo".to_string(), 1)));
}

#[test]
fn enclosing_call_two_args() {
    // (foo bar baz |)
    let result = find_enclosing_call("(foo bar baz )", 0, 13);
    assert_eq!(result, Some(("foo".to_string(), 2)));
}

#[test]
fn enclosing_call_nested() {
    // (foo (bar |)) — cursor inside nested call
    let result = find_enclosing_call("(foo (bar ))", 0, 10);
    assert_eq!(result, Some(("bar".to_string(), 0)));
}

#[test]
fn enclosing_call_after_nested() {
    // (foo (bar 1) |) — cursor after completed nested expr
    let result = find_enclosing_call("(foo (bar 1) )", 0, 13);
    assert_eq!(result, Some(("foo".to_string(), 1)));
}

#[test]
fn enclosing_call_multiline() {
    let src = "(defun add\n  (a b)\n  (+ a b))";
    // cursor on line 2, col 7: inside (+ a |b)
    let result = find_enclosing_call(src, 2, 7);
    assert_eq!(result, Some(("+".to_string(), 1)));
}

#[test]
fn enclosing_call_string_arg() {
    // (foo "hello" |) — string counts as one arg
    let result = find_enclosing_call("(foo \"hello\" )", 0, 13);
    assert_eq!(result, Some(("foo".to_string(), 1)));
}

#[test]
fn enclosing_call_in_vector() {
    // [1 2 |] — inside vector, not a call
    let result = find_enclosing_call("[1 2 ]", 0, 5);
    assert!(result.is_none());
}

#[test]
fn enclosing_call_empty_parens() {
    // (|) — cursor in empty parens, no function name
    let result = find_enclosing_call("()", 0, 1);
    assert!(result.is_none());
}

#[test]
fn enclosing_call_with_comment() {
    // Paren in comment should be ignored
    let src = "; (not a call\n(foo bar )";
    let result = find_enclosing_call(src, 1, 9);
    assert_eq!(result, Some(("foo".to_string(), 1)));
}

#[test]
fn enclosing_call_string_with_paren() {
    // Paren inside string should be ignored
    let src = "(foo \"(not\" bar )";
    let result = find_enclosing_call(src, 0, 16);
    assert_eq!(result, Some(("foo".to_string(), 2)));
}

// ── parse_param_names ────────────────────────────────────────

#[test]
fn param_names_simple() {
    assert_eq!(parse_param_names("(a b c)"), vec!["a", "b", "c"]);
}

#[test]
fn param_names_single() {
    assert_eq!(parse_param_names("(x)"), vec!["x"]);
}

#[test]
fn param_names_variadic() {
    assert_eq!(parse_param_names("(a b . rest)"), vec!["a", "b", "rest"]);
}

#[test]
fn param_names_empty() {
    let result: Vec<String> = parse_param_names("()");
    assert!(result.is_empty());
}

// ── document_symbols_from_ast ────────────────────────────────

#[test]
fn doc_symbols_defun() {
    let src = "(defun foo (x) x)";
    let (ast, span_map, sym_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
    let symbols = document_symbols_from_ast(&ast, &span_map, &sym_spans, &[]);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "foo");
    assert_eq!(symbols[0].kind, SymbolKind::FUNCTION);
    assert!(symbols[0].detail.is_some());
}

#[test]
fn doc_symbols_define_variable() {
    let src = "(define x 42)";
    let (ast, span_map, sym_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
    let symbols = document_symbols_from_ast(&ast, &span_map, &sym_spans, &[]);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "x");
    assert_eq!(symbols[0].kind, SymbolKind::VARIABLE);
    assert!(symbols[0].detail.is_none());
}

#[test]
fn doc_symbols_define_function_shorthand() {
    let src = "(define (square x) (* x x))";
    let (ast, span_map, sym_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
    let symbols = document_symbols_from_ast(&ast, &span_map, &sym_spans, &[]);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "square");
    assert_eq!(symbols[0].kind, SymbolKind::FUNCTION);
}

#[test]
fn doc_symbols_defmacro() {
    let src = "(defmacro unless (test body) `(if (not ,test) ,body))";
    let (ast, span_map, sym_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
    let symbols = document_symbols_from_ast(&ast, &span_map, &sym_spans, &[]);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "unless");
    assert_eq!(symbols[0].kind, SymbolKind::OPERATOR);
}

#[test]
fn doc_symbols_multiple() {
    let src = "(define x 1)\n(defun f (a) a)\n(defmacro m (x) x)";
    let (ast, span_map, sym_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
    let symbols = document_symbols_from_ast(&ast, &span_map, &sym_spans, &[]);
    assert_eq!(symbols.len(), 3);
    assert_eq!(symbols[0].name, "x");
    assert_eq!(symbols[1].name, "f");
    assert_eq!(symbols[2].name, "m");
}

#[test]
fn doc_symbols_no_defs() {
    let src = "(+ 1 2)\n(println \"hello\")";
    let (ast, span_map, sym_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
    let symbols = document_symbols_from_ast(&ast, &span_map, &sym_spans, &[]);
    assert!(symbols.is_empty());
}

#[test]
fn doc_symbols_selection_range_is_name() {
    let src = "(defun foo (x) x)";
    let (ast, span_map, sym_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
    let symbols = document_symbols_from_ast(&ast, &span_map, &sym_spans, &[]);
    assert_eq!(symbols.len(), 1);
    // selection_range should be just "foo", not the whole form
    let sel = symbols[0].selection_range;
    assert_eq!(sel.end.character - sel.start.character, 3); // "foo" = 3 chars
}

// ── extract_params_from_doc ──────────────────────────────────

#[test]
fn extract_params_from_doc_simple() {
    let doc = "Do something.\n\n```sema\n(foo a b c)\n```";
    let params = extract_params_from_doc(doc, "foo");
    assert_eq!(params, Some(vec!["a".into(), "b".into(), "c".into()]));
}

#[test]
fn extract_params_from_doc_rejects_nested_parens() {
    // Compound expressions like (+ 1 2) are not valid parameter names,
    // so the whole call is skipped to avoid nonsensical inlay hints.
    let doc = "Doc.\n\n```sema\n(foo (+ 1 2) y)\n```";
    assert!(extract_params_from_doc(doc, "foo").is_none());
}

#[test]
fn extract_params_from_doc_no_match() {
    let doc = "Doc.\n\n```sema\n(bar a b)\n```";
    assert!(extract_params_from_doc(doc, "foo").is_none());
}

#[test]
fn extract_params_from_doc_no_code_block() {
    let doc = "Just a description, no code.";
    assert!(extract_params_from_doc(doc, "foo").is_none());
}

#[test]
fn extract_params_from_doc_zero_arg_call() {
    // (foo) has no args after the name
    let doc = "Doc.\n\n```sema\n(foo)\n```";
    assert!(extract_params_from_doc(doc, "foo").is_none());
}

#[test]
fn extract_params_from_doc_rejects_string_literal() {
    // String literals are not valid parameter names, so the whole call is skipped.
    let doc = "Doc.\n\n```sema\n(foo \"hello\" x)\n```";
    assert!(extract_params_from_doc(doc, "foo").is_none());
}

#[test]
fn extract_params_from_doc_rejects_higher_order_call() {
    // Higher-order function examples like (map (fn (x) x) lst) must not
    // yield nonsensical hints like "(fn (x) x):" — skip the whole call.
    let doc = "Doc.\n\n```sema\n(map (fn (x) (* x x)) '(1 2 3))\n```";
    assert!(extract_params_from_doc(doc, "map").is_none());
}

#[test]
fn extract_params_from_doc_accepts_all_simple_names() {
    // When every token looks like a parameter name, they are all returned.
    let doc = "Doc.\n\n```sema\n(string/split s sep)\n```";
    let params = extract_params_from_doc(doc, "string/split");
    assert_eq!(params, Some(vec!["s".into(), "sep".into()]));
}

// ── extract_docstring_from_ast (user-fn docstring slice) ─────

fn ast_of(src: &str) -> Vec<sema_core::Value> {
    sema_reader::read_many_with_symbol_spans(src).unwrap().0
}

#[test]
fn docstring_is_leading_string_with_more_body() {
    let ast = ast_of("(defun square (x)\n  \"Return x squared.\"\n  (* x x))");
    assert_eq!(
        extract_docstring_from_ast(&ast, "square"),
        Some("Return x squared.".to_string())
    );
}

#[test]
fn lone_string_body_is_not_a_docstring() {
    // The string is the return value, not documentation.
    let ast = ast_of("(defun greeting () \"hello\")");
    assert_eq!(extract_docstring_from_ast(&ast, "greeting"), None);
}

#[test]
fn docstring_handles_define_shorthand() {
    let ast = ast_of("(define (f x) \"Doc.\" (+ x 1))");
    assert_eq!(
        extract_docstring_from_ast(&ast, "f"),
        Some("Doc.".to_string())
    );
}

// ── find_arg_positions_in_form ───────────────────────────────

#[test]
fn arg_positions_simple() {
    let text = "(foo a b c)";
    let lines: Vec<&str> = text.lines().collect();
    let span = sema_core::Span::new(1, 1, 1, 11);
    let positions = find_arg_positions_in_form(&span, &lines, 3);
    assert_eq!(positions.len(), 3);
    // 'a' at col 5 (0-indexed)
    assert_eq!(positions[0], (0, 5));
    // 'b' at col 7
    assert_eq!(positions[1], (0, 7));
    // 'c' at col 9
    assert_eq!(positions[2], (0, 9));
}

#[test]
fn arg_positions_with_nested() {
    let text = "(foo (+ 1 2) x)";
    let lines: Vec<&str> = text.lines().collect();
    let span = sema_core::Span::new(1, 1, 1, 15);
    let positions = find_arg_positions_in_form(&span, &lines, 2);
    assert_eq!(positions.len(), 2);
    // '(' of nested form at col 5
    assert_eq!(positions[0], (0, 5));
    // 'x' at col 13
    assert_eq!(positions[1], (0, 13));
}

#[test]
fn arg_positions_with_string() {
    let text = "(foo \"hello\" x)";
    let lines: Vec<&str> = text.lines().collect();
    let span = sema_core::Span::new(1, 1, 1, 15);
    let positions = find_arg_positions_in_form(&span, &lines, 2);
    assert_eq!(positions.len(), 2);
    // '"' at col 5
    assert_eq!(positions[0], (0, 5));
    // 'x' at col 13
    assert_eq!(positions[1], (0, 13));
}

#[test]
fn arg_positions_multiline() {
    let text = "(foo\n  a\n  b)";
    let lines: Vec<&str> = text.lines().collect();
    let span = sema_core::Span::new(1, 1, 3, 4);
    let positions = find_arg_positions_in_form(&span, &lines, 2);
    assert_eq!(positions.len(), 2);
    // 'a' on line 1 (0-indexed) col 2
    assert_eq!(positions[0], (1, 2));
    // 'b' on line 2 col 2
    assert_eq!(positions[1], (2, 2));
}

#[test]
fn arg_positions_max_args_limit() {
    let text = "(foo a b c d)";
    let lines: Vec<&str> = text.lines().collect();
    let span = sema_core::Span::new(1, 1, 1, 13);
    let positions = find_arg_positions_in_form(&span, &lines, 2);
    // Only 2 positions even though there are 4 args
    assert_eq!(positions.len(), 2);
}

#[test]
fn arg_positions_no_args() {
    let text = "(foo)";
    let lines: Vec<&str> = text.lines().collect();
    let span = sema_core::Span::new(1, 1, 1, 5);
    let positions = find_arg_positions_in_form(&span, &lines, 5);
    assert!(positions.is_empty());
}

#[test]
fn arg_positions_with_comment() {
    let text = "(foo a ; comment\n  b)";
    let lines: Vec<&str> = text.lines().collect();
    let span = sema_core::Span::new(1, 1, 2, 4);
    let positions = find_arg_positions_in_form(&span, &lines, 2);
    assert_eq!(positions.len(), 2);
    assert_eq!(positions[0], (0, 5)); // 'a'
    assert_eq!(positions[1], (1, 2)); // 'b'
}

// ── top_level_ranges ─────────────────────────────────────────

#[test]
fn top_level_ranges_basic() {
    let src = "(define x 1)\n(defun f (a) a)";
    let (ast, span_map) = sema_reader::read_many_with_spans(src).unwrap();
    let ranges = top_level_ranges(&ast, &span_map, &[]);
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0].0, 0); // first form
    assert_eq!(ranges[1].0, 1); // second form
    assert_eq!(ranges[0].1.start.line, 0);
    assert_eq!(ranges[1].1.start.line, 1);
}

#[test]
fn top_level_ranges_empty() {
    let src = "";
    let (ast, span_map) = sema_reader::read_many_with_spans(src).unwrap();
    let ranges = top_level_ranges(&ast, &span_map, &[]);
    assert!(ranges.is_empty());
}

// ── format_error_message ─────────────────────────────────────

#[test]
fn format_error_with_hint_and_note() {
    // Create an error with hint
    let err = SemaError::eval("test error").with_hint("try this instead");
    let msg = format_error_message(&err);
    assert!(msg.contains("test error"), "msg: {msg}");
    assert!(msg.contains("hint: try this instead"), "msg: {msg}");
}

// ── extract_symbol_at edge cases ──────────────────────────────

#[test]
fn symbol_at_empty_string() {
    assert_eq!(extract_symbol_at("", 0), "");
}

#[test]
fn symbol_at_whitespace_only() {
    assert_eq!(extract_symbol_at("   ", 1), "");
}

#[test]
fn symbol_at_end_of_string() {
    assert_eq!(extract_symbol_at("foo", 3), "foo");
}

#[test]
fn symbol_at_between_parens() {
    assert_eq!(extract_symbol_at("()foo()", 2), "foo");
}

#[test]
fn symbol_at_hash_symbol() {
    // '#' is a valid symbol char in Sema
    assert_eq!(extract_symbol_at("(#t)", 1), "#t");
}

#[test]
fn prefix_empty_string() {
    assert_eq!(extract_prefix("", 0), "");
}

#[test]
fn prefix_only_paren() {
    assert_eq!(extract_prefix("(", 1), "");
}

#[test]
fn prefix_hash_lambda() {
    // '#' and '(' are not symbol chars, so prefix from col 2 is empty
    assert_eq!(extract_prefix("#(+ 1 %)", 2), "");
}

// ── document_symbols edge cases ──────────────────────────────

#[test]
fn doc_symbols_defagent() {
    let src = "(defagent my-agent :model \"claude\")";
    let (ast, span_map, sym_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
    let symbols = document_symbols_from_ast(&ast, &span_map, &sym_spans, &[]);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "my-agent");
    assert_eq!(symbols[0].kind, SymbolKind::CLASS);
}

#[test]
fn doc_symbols_deftool() {
    let src = "(deftool get-weather (location) \"Get weather\" location)";
    let (ast, span_map, sym_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
    let symbols = document_symbols_from_ast(&ast, &span_map, &sym_spans, &[]);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "get-weather");
    assert_eq!(symbols[0].kind, SymbolKind::METHOD);
}

// ── find_enclosing_call edge cases ───────────────────────────

#[test]
fn enclosing_call_deeply_nested() {
    // (a (b (c |))) — cursor at innermost
    let result = find_enclosing_call("(a (b (c )))", 0, 9);
    assert_eq!(result, Some(("c".to_string(), 0)));
}

#[test]
fn enclosing_call_cursor_on_func_name() {
    // Cursor on the function name itself (mid-name), no args yet
    let result = find_enclosing_call("(foo)", 0, 3);
    assert_eq!(result, Some(("fo".to_string(), 0)));
}

#[test]
fn enclosing_call_empty_input() {
    assert!(find_enclosing_call("", 0, 0).is_none());
}

#[test]
fn enclosing_call_escaped_string() {
    // String with escaped quote shouldn't break paren tracking
    let src = r#"(foo "he\"llo" bar )"#;
    let result = find_enclosing_call(src, 0, 19);
    assert_eq!(result, Some(("foo".to_string(), 2)));
}

#[test]
fn enclosing_call_keyword_function() {
    // (:name person) — keyword in call position
    let result = find_enclosing_call("(:name person)", 0, 13);
    assert_eq!(result, Some((":name".to_string(), 0)));
}

// ── utf16_to_byte_offset edge cases ──────────────────────────

#[test]
fn utf16_empty_string() {
    assert_eq!(utf16_to_byte_offset("", 0), 0);
}

#[test]
fn utf16_zero_offset() {
    assert_eq!(utf16_to_byte_offset("hello", 0), 0);
}

#[test]
fn utf16_exact_end() {
    assert_eq!(utf16_to_byte_offset("abc", 3), 3);
}

#[test]
fn utf16_surrogate_pair_middle() {
    // 🌍 is a surrogate pair in UTF-16 (2 code units), 4 bytes in UTF-8
    // "a" = 1 byte, "🌍" = 4 bytes, "b" = 1 byte
    // UTF-16 offset 2 is mid-emoji (second code unit of surrogate pair)
    // The function loops: offset 0→byte 0 ('a', 1 unit), offset 1→byte 1 ('🌍', 2 units),
    // then utf16_count=3 ≥ 2, so it never returns early → falls through to s.len()=6.
    // But actually: at byte_idx=1 ('🌍'), utf16_count is 1 < 2, so it adds 2 → utf16_count=3.
    // Next iteration: byte_idx=5 ('b'), utf16_count=3 ≥ 2 → returns 5.
    let s = "a🌍b";
    assert_eq!(utf16_to_byte_offset(s, 2), 5);
}

// ── parse_param_names edge cases ─────────────────────────────

#[test]
fn param_names_no_parens() {
    // Bare names without parens
    assert_eq!(parse_param_names("a b c"), vec!["a", "b", "c"]);
}

#[test]
fn param_names_with_comments() {
    // Comments in multiline param string
    assert_eq!(parse_param_names("(a ; first\n b)"), vec!["a", "b"]);
}

#[test]
fn param_names_extra_whitespace() {
    assert_eq!(parse_param_names("(  a   b  )"), vec!["a", "b"]);
}

// ── analyze_document edge cases ──────────────────────────────

#[test]
fn analyze_nested_valid_forms() {
    let diags = analyze_document("(defun f (x) (if (> x 0) x (- x)))");
    assert!(diags.is_empty(), "got: {diags:?}");
}

#[test]
fn analyze_multiple_top_level_forms() {
    let src = "(define x 1)\n(define y 2)\n(+ x y)";
    let diags = analyze_document(src);
    assert!(diags.is_empty(), "got: {diags:?}");
}

#[test]
fn analyze_comments_only() {
    let diags = analyze_document("; just a comment\n; another one");
    assert!(diags.is_empty(), "got: {diags:?}");
}

// ── UTF-16 correctness regressions (LSP-1, LSP-2, LSP-4) ─────

// LSP-4: inlay-hint argument positions must be UTF-16 code-unit columns, not
// byte offsets. With a 🎉 (4 UTF-8 bytes, 2 UTF-16 units) before an argument,
// the byte offset diverges from the UTF-16 column.
#[test]
fn inlay_hint_arg_position_uses_utf16_after_emoji() {
    let src = "(defun f (a b) a)\n(f \"🎉\" x)";
    let (mut state, uri) = parsed_state("file:///inlay.sema", src);
    let full = Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 10,
            character: 0,
        },
    };
    let hints = state.handle_inlay_hints(&uri, &full).unwrap();
    // Hint for the second arg `x` (param `b`) on line 1.
    let b_hint = hints
        .iter()
        .find(|h| matches!(&h.label, InlayHintLabel::String(s) if s == "b:"))
        .expect("expected a `b:` inlay hint");
    assert_eq!(b_hint.position.line, 1);
    // `(f "🎉" x)` — `x` is at byte 10 but UTF-16 column 8 (🎉 is 2 units).
    assert_eq!(
        b_hint.position.character, 8,
        "arg position must be a UTF-16 column, not a byte offset"
    );
}

// LSP-2: semantic-token length must be in UTF-16 code units. A user-defined
// name containing an astral char is 1 char wider than its UTF-16 width per
// such char.
#[test]
fn semantic_token_length_is_utf16() {
    // `x𝐀` symbol (𝐀 = U+1D400, alphabetic so a legal symbol char): 2 chars,
    // but 3 UTF-16 code units (𝐀 = 2).
    let src = "(define x𝐀 1)\nx𝐀";
    let (state, uri) = parsed_state("file:///semtok.sema", src);
    let result = state.handle_semantic_tokens_full(&uri).unwrap();
    let SemanticTokensResult::Tokens(tokens) = result else {
        panic!("expected token data");
    };
    // At least one emitted token must report the UTF-16 length (3), never the
    // char length (2).
    assert!(
        tokens.data.iter().any(|t| t.length == 3),
        "expected a token of UTF-16 length 3, got {:?}",
        tokens.data.iter().map(|t| t.length).collect::<Vec<_>>()
    );
}

// LSP-1: completion scope queries must convert the incoming UTF-16
// Position.character to a Sema (char) column. On a line containing an emoji
// before the cursor, the raw `character + 1` would land in the wrong scope
// column. We assert the local binding is still surfaced.
#[test]
fn completion_local_binding_visible_after_emoji_on_line() {
    // `total` is a let binding; the cursor sits after an emoji string on its
    // line, so the editor's UTF-16 character offset diverges from the char
    // column the scope tree expects.
    let src = "(let ((total 5))\n  (+ \"🎉\" to))";
    let (state, uri) = parsed_state("file:///comp.sema", src);
    // Line 1 (0-indexed): `  (+ "🎉" to)`. `to` spans UTF-16 chars 10..12
    // (🎉 = 2 units). Cursor at end of `to`.
    let pos = Position {
        line: 1,
        character: 12,
    };
    let items = state.handle_complete(&uri, &pos);
    assert!(
        items.iter().any(|i| i.label == "total"),
        "expected `total` binding in completions, got {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
}

// LSP-3: named-let detection must require items[2] to be the bindings list.
// A malformed `(let x 5)` (symbol then non-list) must NOT be treated as a
// named let that binds `x`.
#[test]
fn malformed_let_not_misclassified_as_named_let() {
    let src = "(let x 5)";
    let (ast, span_map, symbol_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
    let tree = scope::ScopeTree::build(&ast, &span_map, &symbol_spans);
    // `x` must not be a visible binding anywhere in the (malformed) let body.
    let visible = tree.visible_bindings_at(1, 8);
    assert!(
        !visible.iter().any(|(n, _)| n == "x"),
        "`x` should not be bound: {visible:?}"
    );
}

// LSP-3: a genuine named let still binds both the loop name and its vars.
#[test]
fn named_let_binds_loop_name_and_vars() {
    let src = "(let loop ((i 0))\n  (loop i))";
    let (ast, span_map, symbol_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
    let tree = scope::ScopeTree::build(&ast, &span_map, &symbol_spans);
    let visible = tree.visible_bindings_at(2, 4);
    assert!(
        visible.iter().any(|(n, _)| n == "loop"),
        "loop name should be bound: {visible:?}"
    );
    assert!(
        visible.iter().any(|(n, _)| n == "i"),
        "loop var should be bound: {visible:?}"
    );
}

// ── Navigation handler correctness (references / rename / robustness) ─────────

#[test]
fn rename_and_references_ignore_quoted_symbols() {
    // `foo` appears as code (define + use) and as DATA inside a quoted list. Rename and
    // references must touch ONLY the code occurrences, never the quoted ones (rewriting
    // quoted data silently changes the program's meaning).
    let src = "(define foo 1)\n'(foo bar foo)\n(+ foo 1)";
    let (state, uri) = parsed_state("file:///q.sema", src);
    let pos = Position {
        line: 2,
        character: 3,
    }; // the code `foo` in (+ foo 1)

    let refs = state.handle_references(&uri, &pos);
    assert!(!refs.is_empty(), "should find the code references");
    assert!(
        refs.iter().all(|l| l.range.start.line != 1),
        "quoted foo (line 1) must not be a reference: {refs:?}"
    );

    let edit = state
        .handle_rename(&uri, &pos, "baz")
        .expect("rename a user symbol");
    let edits = &edit.changes.expect("edits")[&uri];
    assert!(
        edits.iter().all(|e| e.range.start.line != 1),
        "rename must not rewrite the quoted foo on line 1: {edits:?}"
    );
    assert!(edits.iter().all(|e| e.new_text == "baz"));
}

#[test]
fn references_top_level_skips_shadowing_param() {
    // A param named `total` shadows the top-level `total` inside `f`. References on the
    // top-level binding must NOT include the shadowing param/use on line 1.
    let src = "(define total 1)\n(defun f (total) total)\n(+ total 1)";
    let (state, uri) = parsed_state("file:///shadow.sema", src);
    let refs = state.handle_references(
        &uri,
        &Position {
            line: 2,
            character: 3,
        },
    );
    assert!(
        refs.iter().all(|l| l.range.start.line != 1),
        "shadowed param scope (line 1) must be excluded: {refs:?}"
    );
}

#[test]
fn navigation_handlers_tolerate_past_eof_and_unopened_uri() {
    // Graceful degradation: a position past EOF and a never-opened URI must not panic.
    let (mut state, uri) = parsed_state("file:///e.sema", "(define x 1)");
    let past = Position {
        line: 999,
        character: 0,
    };
    assert!(state.handle_hover(&uri, &past).is_none());
    assert!(state.handle_document_highlight(&uri, &past).is_none());
    assert!(state.handle_references(&uri, &past).is_empty());
    let ghost = Url::parse("file:///never-opened.sema").unwrap();
    assert!(state
        .handle_references(
            &ghost,
            &Position {
                line: 0,
                character: 0
            }
        )
        .is_empty());
}

#[test]
fn completion_works_on_trailing_empty_line_after_newline() {
    // Cursor on the empty line after a trailing `\n` (where you type a new top-level
    // form) — `str::lines()` drops that line; completion must still fire.
    let (mut state, uri) = parsed_state("file:///t.sema", "(define x 1)\n");
    state
        .documents
        .insert(uri.as_str().to_string(), "(define x 1)\n".to_string());
    let items = state.handle_complete(
        &uri,
        &Position {
            line: 1,
            character: 0,
        },
    );
    assert!(
        !items.is_empty(),
        "completion should offer items on the trailing empty line"
    );
}
