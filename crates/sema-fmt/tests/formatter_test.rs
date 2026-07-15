//! Integration tests for the Sema formatter public API.
use sema_fmt::{format_source, FormatOptions};

fn opts(width: usize, indent: usize, align: bool) -> FormatOptions {
    FormatOptions {
        width,
        indent,
        align,
    }
}

fn fmt(input: &str) -> String {
    format_source(input, &FormatOptions::default()).unwrap()
}

fn fmt_narrow(input: &str) -> String {
    format_source(input, &opts(40, 2, false)).unwrap()
}

fn fmt_aligned(input: &str) -> String {
    format_source(input, &opts(80, 2, true)).unwrap()
}

// 1. Simple atom formatting
#[test]
fn test_simple_form() {
    assert_eq!(fmt("(+ 1 2)"), "(+ 1 2)\n");
}

#[test]
fn test_simple_form_extra_spaces() {
    assert_eq!(fmt("(+   1    2)"), "(+ 1 2)\n");
}

#[test]
fn test_atom() {
    assert_eq!(fmt("42"), "42\n");
}

#[test]
fn test_string() {
    assert_eq!(fmt(r#""hello world""#), "\"hello world\"\n");
}

#[test]
fn test_keyword() {
    assert_eq!(fmt(":name"), ":name\n");
}

#[test]
fn test_boolean() {
    assert_eq!(fmt("#t"), "#t\n");
    assert_eq!(fmt("#f"), "#f\n");
}

// 2. Line breaking
#[test]
fn test_line_break_long_call() {
    let input = "(some-very-long-function-name arg1 arg2 arg3 arg4 arg5 arg6 arg7 arg8 arg9 arg10)";
    let result = fmt(input);
    // Should break since it exceeds 80 chars
    assert!(result.contains('\n'));
    // Should still have matching parens
    assert_eq!(
        result.chars().filter(|c| *c == '(').count(),
        result.chars().filter(|c| *c == ')').count()
    );
}

#[test]
fn test_short_form_stays_one_line() {
    assert_eq!(fmt("(+ 1 2 3)"), "(+ 1 2 3)\n");
}

// 3. Comment preservation
#[test]
fn test_trailing_comment() {
    assert_eq!(fmt("(+ 1 2) ; add"), "(+ 1 2) ; add\n");
}

#[test]
fn test_standalone_comment() {
    assert_eq!(
        fmt("; this is a comment\n(+ 1 2)"),
        "; this is a comment\n(+ 1 2)\n"
    );
}

// 4. Blank line preservation
#[test]
fn test_blank_line_between_forms() {
    let input = "(define x 1)\n\n(define y 2)";
    let result = fmt(input);
    assert_eq!(result, "(define x 1)\n\n(define y 2)\n");
}

#[test]
fn test_multiple_blank_lines_collapsed() {
    let input = "(define x 1)\n\n\n\n(define y 2)";
    let result = fmt(input);
    assert_eq!(result, "(define x 1)\n\n(define y 2)\n");
}

// 5. Define with body indentation
#[test]
fn test_define_body() {
    let input = "(define (factorial n) (if (<= n 1) 1 (* n (factorial (- n 1)))))";
    let result = fmt_narrow(input);
    assert!(result.starts_with("(define (factorial n)"));
    // Body should be indented 2 spaces
    let lines: Vec<&str> = result.lines().collect();
    assert!(
        lines.len() > 1,
        "narrow width should force multi-line output"
    );
    assert!(lines[1].starts_with("  "));
}

#[test]
fn test_defn_body() {
    let result = fmt_narrow("(defn greet [name] (string-append \"Hello, \" name))");
    assert!(result.starts_with("(defn greet [name]"));
}

#[test]
fn test_lambda() {
    let result = fmt_narrow("(lambda (x y) (+ x y) (* x y))");
    let lines: Vec<&str> = result.lines().collect();
    assert!(lines[0].contains("lambda"));
    assert!(
        lines.len() > 1,
        "narrow width should force multi-line output"
    );
    assert!(lines[1].starts_with("  "));
}

// 6. Let with binding indentation
#[test]
fn test_let_binding() {
    let result = fmt_narrow("(let ([x 1] [y 2]) (+ x y))");
    assert!(result.starts_with("(let"));
}

// 7. Cond with clause formatting
#[test]
fn test_cond_clauses() {
    let input = "(cond ((= x 1) \"one\") ((= x 2) \"two\") (else \"other\"))";
    let result = fmt_narrow(input);
    let lines: Vec<&str> = result.lines().collect();
    assert!(lines[0].starts_with("(cond"));
    // Clauses indented 2 from opening paren
    assert!(
        lines.len() > 1,
        "narrow width should force multi-line output"
    );
    assert!(lines[1].starts_with("  "));
}

// 8. Map formatting
#[test]
fn test_map_short() {
    assert_eq!(fmt("{:a 1 :b 2}"), "{:a 1 :b 2}\n");
}

#[test]
fn test_map_long() {
    let input = "{:name \"Alice\" :age 30 :email \"alice@example.com\" :city \"Wonderland\"}";
    let result = fmt_narrow(input);
    // Should break into multiple lines
    assert!(result.contains('\n'));
}

// 9. Vector formatting
#[test]
fn test_vector_short() {
    assert_eq!(fmt("[1 2 3]"), "[1 2 3]\n");
}

#[test]
fn test_vector_long() {
    let input = "[very-long-element-1 very-long-element-2 very-long-element-3 very-long-element-4]";
    let result = fmt_narrow(input);
    assert!(result.contains('\n'));
}

// 10. Quote/quasiquote
#[test]
fn test_quote() {
    assert_eq!(fmt("'(a b c)"), "'(a b c)\n");
}

#[test]
fn test_quasiquote_unquote() {
    assert_eq!(fmt("`(a ,b ,@c)"), "`(a ,b ,@c)\n");
}

// 11. F-string preservation
#[test]
fn test_fstring() {
    assert_eq!(fmt("f\"Hello ${name}!\""), "f\"Hello ${name}!\"\n");
}

// 12. Regex preservation
#[test]
fn test_regex() {
    let input = "#\"\\d+\"";
    let result = fmt(input);
    assert_eq!(result, "#\"\\d+\"\n");
}

#[test]
fn test_regex_in_form() {
    let input = "(regex/match? #\"[a-z]+\" \"hello\")";
    let result = fmt(input);
    assert_eq!(result, "(regex/match? #\"[a-z]+\" \"hello\")\n");
}

// 13. Idempotency
#[test]
fn test_idempotency_simple() {
    let input = "(define (factorial n)\n  (if (<= n 1)\n    1\n    (* n (factorial (- n 1)))))\n";
    let first = fmt(input);
    let second = fmt(&first);
    assert_eq!(first, second, "formatting should be idempotent");
}

#[test]
fn test_idempotency_with_comments() {
    let input = "; header comment\n\n(define x 42) ; the answer\n\n(define y 7)\n";
    let first = fmt(input);
    let second = fmt(&first);
    assert_eq!(
        first, second,
        "formatting with comments should be idempotent"
    );
}

#[test]
fn test_idempotency_multiline() {
    let input = "(some-very-long-function-name argument1 argument2 argument3 argument4 argument5)";
    let first = fmt(input);
    let second = fmt(&first);
    assert_eq!(first, second, "multiline formatting should be idempotent");
}

// 14. Nested forms
#[test]
fn test_nested_forms() {
    assert_eq!(fmt("(+ (* 2 3) (- 4 1))"), "(+ (* 2 3) (- 4 1))\n");
}

#[test]
fn test_deeply_nested() {
    let input = "(a (b (c (d (e 1)))))";
    let result = fmt(input);
    assert_eq!(result, "(a (b (c (d (e 1)))))\n");
}

// 15. Threading macros
#[test]
fn test_threading_short() {
    assert_eq!(fmt("(-> x f g)"), "(-> x f g)\n");
}

#[test]
fn test_threading_long() {
    let input =
        "(-> some-value (map some-function) (filter some-predicate?) (reduce some-reducer 0))";
    let result = fmt_narrow(input);
    let lines: Vec<&str> = result.lines().collect();
    assert!(lines[0].starts_with("(-> some-value"));
    assert!(
        lines.len() > 1,
        "narrow width should force multi-line output"
    );
    assert!(lines[1].starts_with("  "));
}

// Edge cases
#[test]
fn test_empty_input() {
    assert_eq!(format_source("", &FormatOptions::default()).unwrap(), "");
}

#[test]
fn test_whitespace_only() {
    assert_eq!(
        format_source("   \n  \n  ", &FormatOptions::default()).unwrap(),
        ""
    );
}

#[test]
fn test_empty_list() {
    assert_eq!(fmt("()"), "()\n");
}

#[test]
fn test_empty_vector() {
    assert_eq!(fmt("[]"), "[]\n");
}

#[test]
fn test_empty_map() {
    assert_eq!(fmt("{}"), "{}\n");
}

#[test]
fn test_shebang() {
    let input = "#!/usr/bin/env sema\n(+ 1 2)";
    let result = fmt(input);
    assert_eq!(result, "#!/usr/bin/env sema\n(+ 1 2)\n");
}

#[test]
fn test_char_literal() {
    assert_eq!(fmt("#\\a"), "#\\a\n");
    assert_eq!(fmt("#\\space"), "#\\space\n");
    assert_eq!(fmt("#\\newline"), "#\\newline\n");
}

#[test]
fn test_short_lambda() {
    assert_eq!(fmt("#(+ %1 1)"), "#(+ %1 1)\n");
}

#[test]
fn test_dot() {
    assert_eq!(fmt("(a . b)"), "(a . b)\n");
}

#[test]
fn test_no_trailing_whitespace() {
    let input = "(define x 1)   ";
    let result = fmt(input);
    for line in result.lines() {
        assert_eq!(
            line,
            line.trim_end(),
            "line should have no trailing whitespace"
        );
    }
}

#[test]
fn test_trailing_newline() {
    let result = fmt("42");
    assert!(result.ends_with('\n'));
    assert!(!result.ends_with("\n\n"));
}

#[test]
fn test_multiple_top_level_forms() {
    let input = "(define x 1)\n(define y 2)\n(+ x y)";
    let result = fmt(input);
    assert_eq!(result, "(define x 1)\n(define y 2)\n(+ x y)\n");
}

#[test]
fn test_bytevector() {
    assert_eq!(fmt("#u8(1 2 3)"), "#u8(1 2 3)\n");
}

#[test]
fn test_string_escaping_roundtrip() {
    // \n escapes stay as \n escapes (original source preserved)
    let input = "\"hello\\nworld\\t!\"";
    let result = fmt(input);
    assert_eq!(result, "\"hello\\nworld\\t!\"\n");
}

#[test]
fn test_multiline_string_preserved() {
    // Multi-line strings stay multi-line (original source preserved)
    let input = "\"line one\nline two\nline three\"";
    let result = fmt(input);
    assert_eq!(result, "\"line one\nline two\nline three\"\n");
    let result2 = fmt(&result);
    assert_eq!(result, result2, "multi-line string should be idempotent");
}

#[test]
fn test_short_escape_stays_escaped() {
    // Short strings with \n stay escaped
    let input = "(string/join items \"\\n\\n\")";
    let result = fmt(input);
    assert_eq!(result, "(string/join items \"\\n\\n\")\n");
}

#[test]
fn test_match_form() {
    let input = "(match x (1 \"one\") (2 \"two\") (_ \"other\"))";
    let result = fmt_narrow(input);
    let lines: Vec<&str> = result.lines().collect();
    assert!(lines[0].starts_with("(match"));
}

#[test]
fn test_do_form() {
    let result = fmt_narrow("(do (display \"hello\") (display \"world\") (newline))");
    let lines: Vec<&str> = result.lines().collect();
    assert!(lines[0].starts_with("(do"));
    if lines.len() > 1 {
        assert!(lines[1].starts_with("  "));
    }
}

#[test]
fn test_when_form() {
    let result = fmt_narrow("(when (> x 0) (display x) (newline))");
    let lines: Vec<&str> = result.lines().collect();
    assert!(lines[0].starts_with("(when"));
}

#[test]
fn test_if_form_short() {
    assert_eq!(fmt("(if #t 1 0)"), "(if #t 1 0)\n");
}

#[test]
fn test_if_form_long() {
    let input =
        "(if (some-very-long-predicate? x y z) (do-something-complex x) (do-other-thing y))";
    let result = fmt(input);
    let lines: Vec<&str> = result.lines().collect();
    assert!(lines[0].starts_with("(if"));
}

#[test]
fn test_nil() {
    assert_eq!(fmt("nil"), "nil\n");
}

#[test]
fn test_negative_number() {
    assert_eq!(fmt("-42"), "-42\n");
}

#[test]
fn test_float() {
    assert_eq!(fmt("3.14"), "3.14\n");
}

#[test]
fn test_closing_parens_not_on_own_line() {
    let input = "(define (foo x)\n  (+ x 1))";
    let result = fmt(input);
    // No line should be just closing parens
    for line in result.lines() {
        let trimmed = line.trim();
        assert!(
            !trimmed.chars().all(|c| c == ')' || c == ']' || c == '}'),
            "closing delimiters should not be on their own line: {:?}",
            line
        );
    }
}

#[test]
fn test_real_world_hello_sema() {
    let input = r#";; hello.sema — Basic Sema demo

;; Factorial
(define (factorial n)
  (if (<= n 1) 1 (* n (factorial (- n 1)))))

(display "Factorial of 10: ")
(println (factorial 10))

;; Fibonacci
(define (fib n)
  (cond
((= n 0) 0)
((= n 1) 1)
(else (+ (fib (- n 1)) (fib (- n 2))))))

(display "Fibonacci of 10: ")
(println (fib 10))

;; Map, filter, fold
(define numbers (range 1 11))
(display "Numbers: ")
(println numbers)

(define squares (map (lambda (x) (* x x)) numbers))
(display "Squares: ")
(println squares)

(define evens (filter even? numbers))
(display "Evens: ")
(println evens)

(define sum (foldl + 0 numbers))
(display "Sum 1-10: ")
(println sum)"#;
    let result = fmt(input);
    // Should be idempotent
    let result2 = fmt(&result);
    assert_eq!(
        result, result2,
        "real-world formatting should be idempotent"
    );
    // Comments should be preserved
    assert!(result.contains(";; hello.sema"));
    assert!(result.contains(";; Factorial"));
    assert!(result.contains(";; Fibonacci"));
    // Blank lines between sections preserved
    assert!(result.contains("\n\n"));
}

#[test]
fn test_idempotency_cond_multiline() {
    let input = "(cond\n  ((= x 0) 0)\n  ((= x 1) 1)\n  (else (+ (fib (- x 1)) (fib (- x 2)))))\n";
    let first = fmt(input);
    let second = fmt(&first);
    assert_eq!(first, second, "cond formatting should be idempotent");
}

#[test]
fn test_idempotency_let_multiline() {
    let input = "(let ((x 10)\n      (y 20))\n  (+ x y))\n";
    let first = fmt(input);
    let second = fmt(&first);
    assert_eq!(first, second, "let formatting should be idempotent");
}

#[test]
fn test_idempotency_define_function() {
    let input = "(define (factorial n)\n  (if (<= n 1) 1 (* n (factorial (- n 1)))))\n";
    let first = fmt(input);
    let second = fmt(&first);
    assert_eq!(
        first, second,
        "define function formatting should be idempotent"
    );
}

// Bug fix tests: inner comments preserved

#[test]
fn test_inner_comment_in_define() {
    let input = "(define (foo x)\n  ;; compute result\n  (+ x 1))";
    let result = format_source(input, &FormatOptions::default()).unwrap();
    assert!(
        result.contains(";; compute result"),
        "inner comment should be preserved, got: {result}"
    );
    assert!(
        result.contains("(+ x 1)"),
        "body should be preserved, got: {result}"
    );
}

#[test]
fn test_inner_comment_in_let() {
    let input = "(let ((x 1))\n  ;; use x\n  (+ x 2))";
    let result = format_source(input, &FormatOptions::default()).unwrap();
    assert!(
        result.contains(";; use x"),
        "inner comment in let should be preserved, got: {result}"
    );
}

#[test]
fn test_inner_comment_in_cond() {
    let input =
        "(cond\n  ;; first case\n  ((= x 1) \"one\")\n  ;; second case\n  ((= x 2) \"two\"))";
    let result = format_source(input, &FormatOptions::default()).unwrap();
    assert!(
        result.contains(";; first case"),
        "comment before first clause preserved, got: {result}"
    );
    assert!(
        result.contains(";; second case"),
        "comment before second clause preserved, got: {result}"
    );
}

#[test]
fn test_classify_form_non_symbol_head() {
    // (42 define x) should NOT be classified as Body
    let input = "(42 define x)";
    let result = format_source(input, &FormatOptions::default()).unwrap();
    // Should be formatted as a function call, not as a body form
    assert_eq!(result.trim(), "(42 define x)");
}

#[test]
fn test_inner_comment_idempotency() {
    let input = "(define (foo x)\n  ;; compute result\n  (+ x 1))";
    let first = format_source(input, &FormatOptions::default()).unwrap();
    let second = format_source(&first, &FormatOptions::default()).unwrap();
    assert_eq!(
        first, second,
        "formatting with inner comments should be idempotent"
    );
}

// Numeric literal source preservation (integers and floats use raw source text)

#[test]
fn test_integer_preserved() {
    assert_eq!(fmt("42"), "42\n");
    assert_eq!(fmt("-7"), "-7\n");
    assert_eq!(fmt("0"), "0\n");
}

#[test]
fn test_float_preserved() {
    assert_eq!(fmt("3.14"), "3.14\n");
    assert_eq!(fmt("-0.5"), "-0.5\n");
    assert_eq!(fmt("100.0"), "100.0\n");
}

#[test]
fn test_numeric_in_form_preserved() {
    assert_eq!(fmt("(define x 42)"), "(define x 42)\n");
    assert_eq!(fmt("(+ 3.14 2.0)"), "(+ 3.14 2.0)\n");
}

#[test]
fn test_numeric_literal_idempotency() {
    for input in &["42", "-7", "3.14", "-0.5", "100.0"] {
        let first = fmt(input);
        let second = fmt(&first);
        assert_eq!(
            first, second,
            "numeric literal {input} should be idempotent"
        );
    }
}

// Unicode preservation

#[test]
fn test_unicode_in_strings() {
    assert_eq!(fmt("\"héllo wörld\""), "\"héllo wörld\"\n");
    assert_eq!(fmt("\"日本語\""), "\"日本語\"\n");
    assert_eq!(fmt("\"emoji: 🎉\""), "\"emoji: 🎉\"\n");
}

#[test]
fn test_unicode_in_symbols() {
    assert_eq!(fmt("(café 42)"), "(café 42)\n");
}

// Example corpus idempotency

#[test]
fn test_example_corpus_idempotency() {
    let examples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples");

    if !examples_dir.exists() {
        return;
    }

    let mut files_checked = 0;
    for entry in walkdir(examples_dir.to_str().unwrap()) {
        let source = match std::fs::read_to_string(&entry) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let first = match format_source(&source, &FormatOptions::default()) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let second = format_source(&first, &FormatOptions::default())
            .unwrap_or_else(|e| panic!("second format of {entry} failed: {e}"));
        assert_eq!(first, second, "idempotency failed for {entry}");
        files_checked += 1;
    }
    assert!(
        files_checked > 0,
        "should have checked at least one example file"
    );
}

/// Recursively collect all .sema files under a directory.
fn walkdir(dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    walkdir_inner(std::path::Path::new(dir), &mut files);
    files
}

fn walkdir_inner(dir: &std::path::Path, files: &mut Vec<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walkdir_inner(&path, files);
            } else if path.extension().and_then(|e| e.to_str()) == Some("sema") {
                files.push(path.to_string_lossy().to_string());
            }
        }
    }
}

// === Alignment-specific tests ===

#[test]
fn test_aligned_define_group() {
    let input = "(define (make-num n) (hash-map :type :number :value n))\n\
                 (define (make-bool b) (hash-map :type :bool :value b))\n\
                 (define (make-var name) (hash-map :type :var :name name))";
    let result = fmt_aligned(input);
    let lines: Vec<&str> = result.lines().collect();
    assert_eq!(lines.len(), 3);
    // All body expressions should start at the same column
    let body_cols: Vec<usize> = lines.iter().map(|l| l.find("(hash-map").unwrap()).collect();
    assert!(
        body_cols.iter().all(|&c| c == body_cols[0]),
        "bodies should be aligned at same column, got {:?}\n{}",
        body_cols,
        result
    );
}

#[test]
fn test_aligned_define_not_applied_without_flag() {
    let input = "(define (make-num n) (hash-map :type :number :value n))\n\
                 (define (make-bool b) (hash-map :type :bool :value b))\n\
                 (define (make-var name) (hash-map :type :var :name name))";
    let result = fmt(input);
    // Without --align, each define has single-space separation
    for line in result.lines() {
        assert!(
            !line.contains("  (hash-map"),
            "without --align, defines should not be column-aligned: {line}"
        );
    }
}

#[test]
fn test_aligned_define_group_idempotent() {
    let input = "(define (make-num n) (hash-map :type :number :value n))\n\
                 (define (make-bool b) (hash-map :type :bool :value b))\n\
                 (define (make-var name) (hash-map :type :var :name name))";
    let first = fmt_aligned(input);
    let second = fmt_aligned(&first);
    assert_eq!(first, second, "aligned defines should be idempotent");
}

#[test]
fn test_aligned_cond_clauses() {
    let input = "(cond\n  ((= x 1) \"one\")\n  ((= x 100) \"hundred\")\n  (else \"other\"))";
    let result = fmt_aligned(input);
    let lines: Vec<&str> = result.lines().collect();
    // Find the result-expression columns
    let clause_lines: Vec<&str> = lines[1..].to_vec();
    let result_cols: Vec<Option<usize>> = clause_lines.iter().map(|l| l.find('"')).collect();
    // All string results should start at the same column
    let valid_cols: Vec<usize> = result_cols.iter().filter_map(|c| *c).collect();
    if valid_cols.len() >= 2 {
        assert!(
            valid_cols.iter().all(|&c| c == valid_cols[0]),
            "cond results should be aligned, got {:?}\n{}",
            valid_cols,
            result
        );
    }
}

#[test]
fn test_aligned_cond_idempotent() {
    let input = "(cond\n  ((= x 1) \"one\")\n  ((= x 100) \"hundred\")\n  (else \"other\"))";
    let first = fmt_aligned(input);
    let second = fmt_aligned(&first);
    assert_eq!(first, second, "aligned cond should be idempotent");
}

#[test]
fn test_aligned_let_bindings() {
    let input = "(let ((x 1)\n      (longer-name 42)\n      (y 2))\n  (+ x y))";
    let result = fmt_aligned(input);
    let first = fmt_aligned(&result);
    assert_eq!(result, first, "aligned let bindings should be idempotent");
}

#[test]
fn test_aligned_map_values() {
    let input = "(define default-keymap\n  {:mcp \"ctrl-o\"\n   :resume \"ctrl-r\"\n   :palette \"ctrl-k\"\n   :interrupt \"ctrl-c\"})";
    let result = fmt_aligned(input);
    assert_eq!(
        result,
        "(define default-keymap\n  {:mcp        \"ctrl-o\"\n   :resume     \"ctrl-r\"\n   :palette    \"ctrl-k\"\n   :interrupt  \"ctrl-c\"})\n"
    );
    assert_eq!(
        fmt_aligned(&result),
        result,
        "aligned map should be idempotent"
    );
}

#[test]
fn test_map_values_not_aligned_without_flag() {
    let input = "{:mcp \"ctrl-o\"\n :interrupt \"ctrl-c\"}";
    assert_eq!(fmt(input), "{:mcp \"ctrl-o\"\n :interrupt \"ctrl-c\"}\n");
}

#[test]
fn test_aligned_map_two_pairs_minimum() {
    // Two pairs is the smallest map eligible for alignment.
    let input = "{:a 1\n :longer 2}";
    assert_eq!(fmt_aligned(input), "{:a       1\n :longer  2}\n");
}

#[test]
fn test_aligned_map_comment_falls_back() {
    // A comment disables value alignment, but the trailing comment stays
    // on its own pair's line.
    let input = "{:a 1 ;; note\n :bb 2\n :ccc 3}";
    assert_eq!(fmt_aligned(input), "{:a 1 ;; note\n :bb 2\n :ccc 3}\n");
}

#[test]
fn test_aligned_map_equal_width_keys_not_aligned() {
    // All keys the same width: nothing to align, normal spacing.
    let input = "{:aa 1\n :bb 2\n :cc 3}";
    assert_eq!(fmt_aligned(input), "{:aa 1\n :bb 2\n :cc 3}\n");
}

#[test]
fn test_aligned_map_odd_entries_fall_back() {
    let input = "{:a 1\n :bb 2\n :ccc}";
    assert_eq!(fmt_aligned(input), "{:a 1\n :bb 2\n :ccc}\n");
}

#[test]
fn test_aligned_map_width_overflow_falls_back() {
    let input = "{:k 1\n :key \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n :kk 2}";
    let result = format_source(input, &opts(30, 2, true)).unwrap();
    assert_eq!(
        result,
        "{:k 1\n :key \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n :kk 2}\n"
    );
}

#[test]
fn test_aligned_map_close_delimiter_counts_toward_width() {
    // The last aligned line carries the closing `}`; it must fit too.
    // Aligned, the last line would be ` :k      "1234567890123456789"}`
    // = 31 columns: aligns at width 31, falls back at width 30.
    let input = "{:short 1\n :k \"1234567890123456789\"}";
    let aligned = format_source(input, &opts(31, 2, true)).unwrap();
    assert_eq!(aligned, "{:short  1\n :k      \"1234567890123456789\"}\n");
    let fallback = format_source(input, &opts(30, 2, true)).unwrap();
    assert_eq!(fallback, "{:short 1\n :k \"1234567890123456789\"}\n");
}

#[test]
fn test_aligned_map_multiline_string_value_falls_back() {
    // A raw newline inside a string literal would break the value column.
    let input = "{:a \"line1\nline2\"\n :bb 2\n :ccc 3}";
    assert_eq!(
        fmt_aligned(input),
        "{:a \"line1\nline2\"\n :bb 2\n :ccc 3}\n"
    );
}

#[test]
fn test_aligned_map_multiline_nested_value_falls_back() {
    // A value that was already multi-line keeps its layout instead of
    // being flattened into an aligned column.
    let input = "{:a {:x 1\n     :yy 2}\n :bb 2\n :ccc 3}";
    let result = fmt_aligned(input);
    assert!(
        !result.contains(":a    "),
        "multi-line nested value should not be aligned:\n{result}"
    );
    assert_eq!(fmt_aligned(&result), result, "should be idempotent");
}

#[test]
fn test_aligned_map_unicode_keys_align_by_chars() {
    // `:naïve` is 7 bytes but 6 chars; padding must use display columns.
    let input = "{:naïve 1\n :bb 22\n :ccccc 3}";
    let result = fmt_aligned(input);
    assert_eq!(result, "{:naïve  1\n :bb     22\n :ccccc  3}\n");
    assert_eq!(fmt_aligned(&result), result, "should be idempotent");
}

#[test]
fn test_aligned_let_multiline_string_falls_back() {
    // Same raw-newline hazard in the aligned binding-group path.
    let input = "(let ((x \"a\nb\")\n      (longer 2))\n  x)";
    let result = fmt_aligned(input);
    assert_eq!(result, "(let ((x \"a\nb\")\n      (longer 2))\n  x)\n");
}

// Issue #114: a trailing comment must stay on its form's line — detaching it
// to a standalone line below silently re-attaches it to the NEXT form.

#[test]
fn test_trailing_comment_stays_in_aligned_define_group() {
    let input = "(define *rows* 24)\n(define *cols* 80)\n(define *cursor* 0)         ;; caret index\n(define *scroll* 0)         ;; lines scrolled";
    let result = fmt_aligned(input);
    assert_eq!(
        result,
        "(define *rows*    24)\n(define *cols*    80)\n(define *cursor*  0)   ;; caret index\n(define *scroll*  0)   ;; lines scrolled\n"
    );
    assert_eq!(fmt_aligned(&result), result, "should be idempotent");
}

#[test]
fn test_trailing_comment_mid_aligned_define_group() {
    // A trailing comment on an earlier define must not break the group
    // or migrate to another line.
    let input = "(define a 1) ;; first\n(define bb 2)\n(define ccc 3)";
    let result = fmt_aligned(input);
    assert_eq!(
        result,
        "(define a    1)  ;; first\n(define bb   2)\n(define ccc  3)\n"
    );
    assert_eq!(fmt_aligned(&result), result, "should be idempotent");
}

#[test]
fn test_trailing_comment_stays_on_map_pair() {
    let input = "(define default-config\n  {:model \"\"        ;; auto-detect\n   :max-turns 50})";
    let result = fmt(input);
    assert_eq!(
        result,
        "(define default-config\n  {:model \"\" ;; auto-detect\n   :max-turns 50})\n"
    );
    assert_eq!(fmt(&result), result, "should be idempotent");
}

#[test]
fn test_standalone_comment_in_map_keeps_own_line() {
    // A comment on its own line documents the pair below — leave it there.
    let input = "{:a 1\n ;; note\n :b 2}";
    assert_eq!(fmt(input), "{:a 1\n ;; note\n :b 2}\n");
}

#[test]
fn test_trailing_comment_stays_in_list_body() {
    let input = "(do\n  (foo) ;; hi\n  (bar))";
    assert_eq!(fmt(input), "(do\n  (foo) ;; hi\n  (bar))\n");
}

#[test]
fn test_trailing_comment_stays_on_let_binding() {
    let input = "(let ((x 1) ;; the x\n      (longer 2))\n  x)";
    assert_eq!(
        fmt(input),
        "(let ((x 1) ;; the x\n      (longer 2))\n  x)\n"
    );
}

#[test]
fn test_comment_between_map_key_and_value() {
    // The value must not be emitted into the comment's line (it would be
    // absorbed into the comment text and vanish from the map).
    let input = "{:a ;; c\n 1\n :b 2}";
    let result = fmt(input);
    assert_eq!(result, "{:a ;; c\n   1\n :b 2}\n");
    assert_eq!(fmt(&result), result, "should be idempotent");
    assert!(
        format_source(&result, &FormatOptions::default()).is_ok(),
        "output should reparse"
    );
}

#[test]
fn test_comment_before_map_close_delimiter() {
    // The closing brace must not be emitted into the comment's line (the
    // output would no longer parse).
    let input = "{:a 1\n :b 2 ;; last\n}";
    let result = fmt(input);
    assert_eq!(result, "{:a 1\n :b 2 ;; last\n }\n");
    assert_eq!(fmt(&result), result, "should be idempotent");
    assert!(
        format_source(&result, &FormatOptions::default()).is_ok(),
        "output should reparse"
    );
}

#[test]
fn test_comment_before_list_close_delimiter() {
    let input = "(do\n  (foo)\n  ;; c\n  )";
    let result = fmt(input);
    assert_eq!(result, "(do\n  (foo)\n  ;; c\n  )\n");
    assert!(
        format_source(&result, &FormatOptions::default()).is_ok(),
        "output should reparse"
    );
}

#[test]
fn test_aligned_map_prefixed_values() {
    let input = "{:a 'foo\n :bb \"x\"\n :ccc 3}";
    let result = fmt_aligned(input);
    assert_eq!(result, "{:a    'foo\n :bb   \"x\"\n :ccc  3}\n");
    assert_eq!(fmt_aligned(&result), result, "should be idempotent");
}

#[test]
fn test_define_group_broken_by_blank_line() {
    let input = "(define x 1)\n\n(define y 2)";
    let result = fmt_aligned(input);
    // Blank line should prevent alignment grouping
    assert_eq!(result, "(define x 1)\n\n(define y 2)\n");
}

#[test]
fn test_single_define_not_aligned() {
    // A single define should not try alignment
    assert_eq!(fmt_aligned("(define x 1)"), "(define x 1)\n");
}

#[test]
fn test_alignment_visual_output() {
    let cases = vec![
        (
            "Aligned defines",
            "(define (make-num n) (hash-map :type :number :value n))\n\
             (define (make-bool b) (hash-map :type :bool :value b))\n\
             (define (make-var name) (hash-map :type :var :name name))\n\
             (define (make-binop op l r) (hash-map :type :binop :op op :left l :right r))\n\
             (define (make-if c t f) (hash-map :type :if :cond c :then t :else f))",
        ),
        (
            "Aligned cond",
            "(cond\n  ((= x 1) \"one\")\n  ((= x 2) \"two\")\n  ((= x 100) \"hundred\")\n  (else \"other\"))",
        ),
        (
            "FizzBuzz cond",
            "(cond\n  ((= 0 (math/remainder i 15)) \"FizzBuzz\")\n  ((= 0 (math/remainder i 3)) \"Fizz\")\n  ((= 0 (math/remainder i 5)) \"Buzz\")\n  (else i))",
        ),
        (
            "Let bindings",
            "(let ((x 1)\n      (longer-name 42)\n      (y 2))\n  (+ x y))",
        ),
    ];

    for (name, input) in &cases {
        let result = fmt_aligned(input);
        eprintln!("\n=== {} ===", name);
        eprint!("{}", result);
    }
}

fn fmt_indent(input: &str, indent: usize) -> String {
    format_source(input, &opts(80, indent, false)).unwrap()
}

// ---------------------------------------------------------------
// Custom indent size
// ---------------------------------------------------------------

#[test]
fn test_indent_1_body_form() {
    let input = "(define (foo x)\n (+ x 1))";
    let result = fmt_indent(input, 1);
    assert_eq!(result, "(define (foo x)\n (+ x 1))\n");
}

#[test]
fn test_indent_4_body_form() {
    let input = "(define (foo x) (+ x 1))";
    let result = fmt_indent(input, 4);
    // Should still fit on one line at width 80
    assert_eq!(result, "(define (foo x) (+ x 1))\n");
}

#[test]
fn test_indent_4_multiline_body() {
    let input = "(define (a-long-function-name some-parameter)\n  (do-something some-parameter)\n  (do-another-thing some-parameter))";
    let result = fmt_indent(input, 4);
    assert_eq!(
        result,
        "(define (a-long-function-name some-parameter)\n    (do-something some-parameter)\n    (do-another-thing some-parameter))\n"
    );
}

#[test]
fn test_indent_4_nested() {
    let input = "(define (f x)\n  (when (> x 0)\n    (println x)))";
    let result = fmt_indent(input, 4);
    assert_eq!(
        result,
        "(define (f x)\n    (when (> x 0)\n        (println x)))\n"
    );
}

#[test]
fn test_indent_1_nested() {
    let input = "(define (f x)\n  (when (> x 0)\n    (println x)))";
    let result = fmt_indent(input, 1);
    assert_eq!(result, "(define (f x)\n (when (> x 0)\n  (println x)))\n");
}

#[test]
fn test_indent_4_let_form() {
    let input = "(let ((x 1)\n      (y 2))\n  (+ x y))";
    let result = fmt_indent(input, 4);
    assert_eq!(result, "(let ((x 1)\n      (y 2))\n    (+ x y))\n");
}

#[test]
fn test_indent_4_cond_form() {
    let input = "(cond\n  ((= x 1) \"one\")\n  ((= x 2) \"two\")\n  (else \"other\"))";
    let result = fmt_indent(input, 4);
    assert_eq!(
        result,
        "(cond\n    ((= x 1) \"one\")\n    ((= x 2) \"two\")\n    (else \"other\"))\n"
    );
}

#[test]
fn test_indent_4_threading() {
    let input = "(-> x\n  (foo)\n  (bar)\n  (baz))";
    let result = fmt_indent(input, 4);
    assert_eq!(result, "(-> x\n    (foo)\n    (bar)\n    (baz))\n");
}

#[test]
fn test_indent_4_if_form() {
    // Narrow width to force multi-line
    let result = format_source(
        "(if (some-long-condition? x) (do-something x) (do-other x))",
        &opts(40, 4, false),
    )
    .unwrap();
    assert!(
        result.contains("\n    "),
        "indent 4 should produce 4-space body indent:\n{}",
        result
    );
}

#[test]
fn test_indent_default_is_2() {
    // Verify format_source uses indent=2 by default
    let with_default =
        format_source("(define (f x)\n  (+ x 1))", &FormatOptions::default()).unwrap();
    let with_explicit = format_source("(define (f x)\n  (+ x 1))", &opts(80, 2, false)).unwrap();
    assert_eq!(with_default, with_explicit);
}

#[test]
fn test_indent_idempotent_4() {
    let input =
        "(define (f x)\n    (when (> x 0)\n        (println x)\n        (println (+ x 1))))";
    let first = fmt_indent(input, 4);
    let second = fmt_indent(&first, 4);
    assert_eq!(first, second, "indent=4 should be idempotent");
}

#[test]
fn test_indent_idempotent_1() {
    let input = "(define (f x)\n (when (> x 0)\n  (println x)))";
    let first = fmt_indent(input, 1);
    let second = fmt_indent(&first, 1);
    assert_eq!(first, second, "indent=1 should be idempotent");
}

#[test]
fn test_indent_4_fn_lambda() {
    let input = "(fn (x y)\n  (+ x y))";
    let result = fmt_indent(input, 4);
    assert_eq!(result, "(fn (x y)\n    (+ x y))\n");
}

#[test]
fn test_indent_4_do_block() {
    let input = "(do\n  (println \"a\")\n  (println \"b\")\n  (println \"c\"))";
    let result = fmt_indent(input, 4);
    assert_eq!(
        result,
        "(do\n    (println \"a\")\n    (println \"b\")\n    (println \"c\"))\n"
    );
}

#[test]
fn test_indent_4_defn() {
    let input = "(defn add (a b)\n  (+ a b))";
    let result = fmt_indent(input, 4);
    assert_eq!(result, "(defn add (a b)\n    (+ a b))\n");
}

#[test]
fn test_indent_4_import() {
    // Import forms should also use custom indent
    let input = "(import\n  \"math\"\n  \"strings\")";
    let result = fmt_indent(input, 4);
    assert_eq!(result, "(import\n    \"math\"\n    \"strings\")\n");
}

#[test]
fn test_indent_4_call() {
    // Regular call that overflows
    let result = format_source(
        "(some-function-with-long-name argument-one argument-two argument-three argument-four)",
        &opts(50, 4, false),
    )
    .unwrap();
    assert!(result.contains("\n"), "should break to multi-line");
}

#[test]
fn test_indent_4_aligned() {
    // Verify indent and align can be combined
    let input = "(define x 1)\n(define longer-name 2)\n(define z 3)";
    let result = format_source(input, &opts(80, 4, true)).unwrap();
    // Alignment should still work with custom indent
    assert!(result.contains("(define x"), "should contain defines");
    let first = format_source(&result, &opts(80, 4, true)).unwrap();
    assert_eq!(result, first, "indent=4 + align should be idempotent");
}

#[test]
fn test_indent_various_sizes() {
    // Test that different indent sizes produce different output for multiline forms
    let input = "(define (f x)\n  (+ x 1))";
    let i1 = fmt_indent(input, 1);
    let i2 = fmt_indent(input, 2);
    let i4 = fmt_indent(input, 4);

    assert!(i1.contains("\n "), "indent 1 should have 1-space indent");
    assert!(i2.contains("\n  "), "indent 2 should have 2-space indent");
    assert!(i4.contains("\n    "), "indent 4 should have 4-space indent");
}

// ---------------------------------------------------------------
// FormatOptions API coverage
// ---------------------------------------------------------------

#[test]
fn test_opts_empty_input() {
    assert_eq!(format_source("", &opts(80, 2, false)).unwrap(), "");
}

#[test]
fn test_opts_whitespace_only() {
    assert_eq!(
        format_source("   \n  \n  ", &opts(80, 2, false)).unwrap(),
        ""
    );
}

#[test]
fn test_opts_width_narrow() {
    let result = format_source("(+ 1 2 3 4 5 6 7 8 9 10)", &opts(15, 2, false)).unwrap();
    assert!(
        result.contains("\n"),
        "narrow width should force line break"
    );
}

#[test]
fn test_opts_width_very_wide() {
    let long_form = "(define (my-function a b c d e f) (+ a b c d e f))";
    let result = format_source(long_form, &opts(200, 2, false)).unwrap();
    assert!(
        !result.trim().contains('\n'),
        "wide width should keep on one line"
    );
}

#[test]
fn test_opts_align_false_no_alignment() {
    let input = "(define x 1)\n(define longer-name 2)";
    let result = format_source(input, &opts(80, 2, false)).unwrap();
    // Without align, defines should NOT have extra padding
    assert!(result.contains("(define x 1)"), "no alignment padding");
    assert!(
        result.contains("(define longer-name 2)"),
        "no alignment padding"
    );
}

#[test]
fn test_opts_shebang_preserved() {
    let input = "#!/usr/bin/env sema\n(println \"hello\")";
    let result = format_source(input, &opts(80, 4, false)).unwrap();
    assert!(
        result.starts_with("#!/usr/bin/env sema\n"),
        "shebang should be preserved"
    );
}

// FMT-1: the trailing-whitespace cleanup must not mangle a CRLF (or bare CR)
// that lives inside a preserved string literal.
#[test]
fn test_crlf_inside_string_literal_preserved() {
    // A literal CR+LF inside the string contents (not an escape sequence).
    let input = "(define x \"foo\r\nbar\")";
    let result = fmt(input);
    assert!(
        result.contains("foo\r\nbar"),
        "CRLF inside string literal must be preserved, got {result:?}"
    );
}

#[test]
fn test_bare_cr_inside_string_literal_preserved() {
    let input = "(define x \"foo\rbar\")";
    let result = fmt(input);
    assert!(
        result.contains("foo\rbar"),
        "bare CR inside string literal must be preserved, got {result:?}"
    );
}

#[test]
fn test_trailing_space_before_cr_in_string_preserved() {
    // A space immediately before a CRLF inside a string is real string content
    // and must not be stripped (the CR ends the trailing-whitespace run).
    let input = "(define x \"foo \r\nbar\")";
    let result = fmt(input);
    assert!(
        result.contains("foo \r\nbar"),
        "space before CRLF inside string must be preserved, got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Comment preservation in "distinguished first line" regions: a comment
// between a form's head and the elements a layout would flatten onto the
// first line must never be deleted.
// ---------------------------------------------------------------------------

#[test]
fn test_comment_after_cond_clause_test_kept() {
    // Real-world case (sema-coder): comment after a clause's test.
    let input = "(cond\n  ((nil? x) ;; EOF\n   (a)\n   (b))\n  (else 2))";
    let result = fmt(input);
    assert!(result.contains(";; EOF"), "comment deleted:\n{result}");
    assert_eq!(fmt(&result), result, "should be idempotent");
}

#[test]
fn test_comment_after_unknown_head_kept() {
    // Real-world case (async-everything.sema): comment after a call head.
    let input = "(async ; fast\n  (sleep 10)\n  (send ch))";
    let result = fmt(input);
    assert_eq!(result, "(async ; fast\n  (sleep 10)\n  (send ch))\n");
}

#[test]
fn test_comment_positions_never_dropped() {
    // One probe per specialized layout's first-line region.
    let cases = [
        "( ;; before-head\n foo bar baz-long-enough-to-not-fit-the-line-with-all-of-this-here)",
        "(let ;; c\n  ((x 1))\n  (body))",
        "(define x ;; c\n  (some-value))",
        "(if ;; c\n  (pred?)\n  1\n  2)",
        "(-> ;; c\n  x\n  (f)\n  (g))",
        "(hash-map ;; c\n  :a 1\n  :b 2)",
        "(hash-map :a 1 ;; note\n  :b 2)",
        "(assoc m ;; c\n  :a 1)",
        "'( ;; c\n a b)",
    ];
    for input in cases {
        for (name, out) in [("fmt", fmt(input)), ("fmt_aligned", fmt_aligned(input))] {
            assert_eq!(
                input.matches(';').count(),
                out.matches(';').count(),
                "{name} dropped a comment for {input:?}:\n{out}"
            );
            assert!(
                format_source(&out, &FormatOptions::default()).is_ok(),
                "{name} output unparseable for {input:?}:\n{out}"
            );
        }
    }
}

#[test]
fn test_aligned_clause_with_inner_comment_falls_back() {
    // The aligned cond path flattens clauses — a comment INSIDE one must
    // force the fallback, not vanish.
    let input = "(cond\n  ((= x 1) ;; one\n   \"one\")\n  ((= x 100) \"hundred\"))";
    let result = fmt_aligned(input);
    assert!(result.contains(";; one"), "comment deleted:\n{result}");
}

#[test]
fn test_aligned_define_with_inner_comment_not_grouped() {
    let input = "(define a 1)\n(define x ;; why\n  2)\n(define ccc 3)";
    let result = fmt_aligned(input);
    assert!(result.contains(";; why"), "comment deleted:\n{result}");
    assert_eq!(fmt_aligned(&result), result, "should be idempotent");
}

// ---------------------------------------------------------------------------
// --align idempotency and layout preservation
// ---------------------------------------------------------------------------

#[test]
fn test_blank_line_after_unaligned_define_group_kept() {
    // Equal-width defines don't align; the batch path must still terminate
    // its last line before the following blank line is emitted.
    let input = "(define a 1)\n(define b 2)\n\n(define (f x)\n  (g x))";
    let result = fmt_aligned(input);
    assert_eq!(
        result,
        "(define a 1)\n(define b 2)\n\n(define (f x)\n  (g x))\n"
    );
    assert_eq!(fmt_aligned(&result), result, "should be idempotent");
}

#[test]
fn test_align_does_not_collapse_multiline_fn_define() {
    // A function define with its body on its own line keeps that layout.
    let input = "(define a 1)\n(define bb 2)\n(define (f x)\n  (g x))";
    let result = fmt_aligned(input);
    assert_eq!(
        result,
        "(define a   1)\n(define bb  2)\n(define (f x)\n  (g x))\n"
    );
    assert_eq!(fmt_aligned(&result), result, "should be idempotent");
}

#[test]
fn test_align_joins_multiline_value_define_once() {
    // format_body joins (define name value) onto one line when it fits, so
    // alignment must treat it as eligible on the FIRST pass.
    let input = "(define hub (channel/new 64))\n(define senders\n  (map f (range 1 201)))";
    let result = fmt_aligned(input);
    assert_eq!(
        result,
        "(define hub      (channel/new 64))\n(define senders  (map f (range 1 201)))\n"
    );
    assert_eq!(fmt_aligned(&result), result, "should be idempotent");
}

#[test]
fn test_aligned_define_subruns_split_at_wide_member() {
    // A member too wide for the shared column is formatted normally and
    // splits the alignment run instead of failing the whole group.
    let input = "(define (gen/int lo hi) (fn () (rand lo hi)))\n(define (gen/nat) (gen/int 0 1000))\n(define (gen/char) (fn () (very-long-call-that-makes-this-line-exceed-eighty-columns 32 126)))\n(define (gen/bool) (fn () (rand 0 1)))\n(define (g) (h))";
    let result = fmt_aligned(input);
    let second = fmt_aligned(&result);
    assert_eq!(result, second, "sub-run alignment should be idempotent");
    // The two defines before the wide member align with each other...
    assert!(
        result.contains("(define (gen/int lo hi)  (fn () (rand lo hi)))"),
        "first sub-run not aligned:\n{result}"
    );
    // ...and the two after it align with each other.
    assert!(
        result.contains("(define (gen/bool)  (fn () (rand 0 1)))"),
        "second sub-run not aligned:\n{result}"
    );
}

#[test]
fn test_def_family_aligns() {
    // `def` is part of the define family.
    let input = "(def x 1)\n(def longer 2)";
    assert_eq!(fmt_aligned(input), "(def x       1)\n(def longer  2)\n");
    let input2 = "(defn f (x) (+ x 1))\n(defn gg (x) (* x 2))";
    assert_eq!(
        fmt_aligned(input2),
        "(defn f (x)   (+ x 1))\n(defn gg (x)  (* x 2))\n"
    );
}

// ---------------------------------------------------------------------------
// Robustness
// ---------------------------------------------------------------------------

#[test]
fn test_deep_nesting_errors_gracefully() {
    // Must return an error, not overflow the stack.
    let deep = format!("{}1{}", "(list ".repeat(2000), ")".repeat(2000));
    let err = format_source(&deep, &FormatOptions::default()).unwrap_err();
    assert!(err.to_string().contains("nested too deeply"), "{err}");
}

#[test]
fn test_moderately_deep_nesting_formats() {
    let deep = format!("{}1{}", "(list ".repeat(100), ")".repeat(100));
    let result = fmt(&deep);
    assert_eq!(fmt(&result), result, "should be idempotent");
}

// ---------------------------------------------------------------------------
// Delimiter preservation in aligned pairs: a [..] pair must never be
// re-emitted as (..) — that turns a vector literal into a call form.
// ---------------------------------------------------------------------------

#[test]
fn test_aligned_vector_pairs_keep_brackets() {
    let input = "[[:a 1]\n [:bb 22]\n [:ccc 333]]";
    let result = fmt_aligned(input);
    assert_eq!(result, "[[:a    1]\n [:bb   22]\n [:ccc  333]]\n");
    assert_eq!(fmt_aligned(&result), result, "should be idempotent");
}

#[test]
fn test_aligned_let_vector_bindings_keep_brackets() {
    let input = "(let [[x 1]\n      [longer 22]]\n  x)";
    let result = fmt_aligned(input);
    assert_eq!(result, "(let [[x       1]\n      [longer  22]]\n  x)\n");
}

#[test]
fn test_aligned_case_vector_clauses_keep_brackets() {
    let input = "(case x\n  [1 \"one\"]\n  [22 \"twotwo\"])";
    let result = fmt_aligned(input);
    assert_eq!(result, "(case x\n  [1   \"one\"]\n  [22  \"twotwo\"])\n");
}

// ---------------------------------------------------------------------------
// Special-form coverage: every special form in the canonical list
// (crates/sema-docs/entries/special-forms/) gets its intended layout.
// ---------------------------------------------------------------------------

#[test]
fn test_case_and_match_subject_on_head_line() {
    let result =
        fmt_narrow("(case x (1 \"one\") (2 \"two\") (else \"other\") (more-padding \"xx\"))");
    assert!(
        result.starts_with("(case x\n"),
        "case subject should share the head line:\n{result}"
    );
    let result = fmt_narrow("(match value ((list a b) (+ a b)) (_ 0) (long-pattern \"yyy\"))");
    assert!(
        result.starts_with("(match value\n"),
        "match subject should share the head line:\n{result}"
    );
}

#[test]
fn test_case_comment_before_subject_kept() {
    let input = "(case ;; pick\n  x\n  (1 \"one\")\n  (2 \"two\"))";
    let result = fmt(input);
    assert!(result.contains(";; pick"), "comment deleted:\n{result}");
    assert_eq!(fmt(&result), result, "should be idempotent");
}

#[test]
fn test_progn_and_async_format_as_body_forms() {
    // progn is an alias of begin, async wraps a body — both put every body
    // expression on its own line instead of pulling the first beside the head.
    assert_eq!(
        fmt_narrow("(progn (step-one arg) (step-two arg) (step-three arg))"),
        "(progn\n  (step-one arg)\n  (step-two arg)\n  (step-three arg))\n"
    );
    assert_eq!(
        fmt_narrow("(async (do-something arg) (do-more arg) (do-even-more arg))"),
        "(async\n  (do-something arg)\n  (do-more arg)\n  (do-even-more arg))\n"
    );
}

#[test]
fn test_special_form_first_line_shapes() {
    // Each form keeps its spec/signature on the head line, body below.
    let cases = [
        (
            "(dotimes (i 10) (print-a-thing i) (another-line i))",
            "(dotimes (i 10)\n",
        ),
        (
            "(for-range (i 0 100) (do-thing-with i) (and-another i))",
            "(for-range (i 0 100)\n",
        ),
        (
            "(for-fold (acc 0) (x xs) (+ acc (weight-of x)))",
            "(for-fold (acc 0) (x xs)\n",
        ),
        (
            "(guard (e (else (handle e))) (risky-thing arg) (more-risky arg))",
            "(guard (e (else (handle e)))\n",
        ),
        (
            "(defmethod area :circle (c) (* 3.14 (sq (radius-of c))))",
            "(defmethod area :circle (c)\n",
        ),
        (
            "(define-values (q r) (floor/ some-numerator some-denominator))",
            "(define-values (q r)\n",
        ),
        (
            "(parameterize ((param val)) (body-one arg) (body-two arg))",
            "(parameterize ((param val))\n",
        ),
        (
            "(with-open-file (f \"path.txt\") (read-line-from f) (another f))",
            "(with-open-file (f \"path.txt\")\n",
        ),
        (
            "(module my-module (export a b) (define aa 1) (define bb 2))",
            "(module my-module\n",
        ),
    ];
    for (input, expected_first_line) in cases {
        let result = fmt_narrow(input);
        assert!(
            result.starts_with(expected_first_line),
            "for {input:?}\nexpected start: {expected_first_line:?}\ngot:\n{result}"
        );
        assert_eq!(fmt_narrow(&result), result, "not idempotent: {input:?}");
    }
}

#[test]
fn test_special_forms_all_idempotent_and_comment_safe() {
    // Canonical special-form list (from crates/sema-docs/entries/special-forms/),
    // each with a comment in the body to exercise comment preservation.
    let forms = [
        "(and a b)",
        "(or a b)",
        "(if p 1 2)",
        "(when p ;; c\n  (a))",
        "(unless p ;; c\n  (a))",
        "(cond (p 1) ;; c\n  (else 2))",
        "(case x (1 \"a\") ;; c\n  (else \"b\"))",
        "(match v (_ 0) ;; c\n  (p 1))",
        "(match* (a b) ((1 2) \"x\"))",
        "(let ((x 1)) ;; c\n  x)",
        "(let* ((x 1)) x)",
        "(letrec ((f (fn () 1))) (f))",
        "(let-values (((a b) (two-values))) a)",
        "(let*-values (((a b) (two-values))) a)",
        "(when-let ((x (find))) x)",
        "(if-let ((x (find))) x 0)",
        "(define x 1)",
        "(def x 1)",
        "(defn f (x) x)",
        "(defun f (x) x)",
        "(defmacro m (x) x)",
        "(defmulti area :shape)",
        "(defmethod area :circle (c) 3.14)",
        "(define-values (a b) (vals))",
        "(fn (x) ;; c\n  x)",
        "(lambda (x) x)",
        "(do (a) ;; c\n  (b))",
        "(begin (a) (b))",
        "(progn (a) (b))",
        "(async (a) ;; c\n  (b))",
        "(await p)",
        "(delay (compute))",
        "(force p)",
        "(while p ;; c\n  (a))",
        "(dotimes (i 3) (p i))",
        "(for (x xs) (p x))",
        "(for-range (i 0 9) (p i))",
        "(for-list (x xs) (f x))",
        "(for-map (x xs) (f x))",
        "(for-filter (x xs) (pred? x))",
        "(for-fold (acc 0) (x xs) (+ acc x))",
        "(guard (e (else 0)) (risky))",
        "(parameterize ((p v)) (body))",
        "(try (a) ;; c\n  (catch e (h e)))",
        "(throw (make-error))",
        "(quote (a b))",
        "(quasiquote (a (unquote b)))",
        "(set! x 2)",
        "(eval '(+ 1 2))",
        "(macroexpand '(when p 1))",
        "(import \"mod\")",
        "(load \"file.sema\")",
        "(export a b)",
        "(module m (export a) (define a 1))",
        "(-> x (f) ;; c\n  (g))",
        "(->> x (f) (g))",
        "(as-> x it (f it))",
        "(some-> x (f))",
        "(prompt \"hi\")",
        "(message :user \"hi\")",
        "(deftool t \"doc\" {:a :string} (body))",
        "(defagent a \"doc\" :tools [t] (body))",
    ];
    for input in forms {
        for opts in [
            FormatOptions::default(),
            opts(80, 2, true),
            opts(30, 2, false),
        ] {
            let first = format_source(input, &opts)
                .unwrap_or_else(|e| panic!("format failed for {input:?}: {e}"));
            let second = format_source(&first, &opts)
                .unwrap_or_else(|e| panic!("reformat failed for {input:?}: {e}\n{first}"));
            assert_eq!(second, first, "not idempotent for {input:?}");
            assert_eq!(
                input.matches(';').count(),
                first.matches(';').count(),
                "comment dropped for {input:?}:\n{first}"
            );
        }
    }
}

#[test]
fn test_bytevector_wraps_at_width() {
    // A single-line literal too long for the width wraps greedily.
    let input = format!(
        "#u8({})",
        (0..32).map(|i| i.to_string()).collect::<Vec<_>>().join(" ")
    );
    let result = fmt(&input);
    assert_eq!(
        result,
        "#u8(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28\n    29 30 31)\n"
    );
    assert_eq!(fmt(&result), result, "wrapping should be idempotent");
}

#[test]
fn test_bytevector_preserves_user_rows() {
    // A hand-arranged grid keeps its row structure; spacing is normalized.
    let input = "#u8( 1  2  3  4\n     5  6  7  8\n     9 10 11 12\n    13 14 15 16)";
    let result = fmt(input);
    assert_eq!(
        result,
        "#u8(1 2 3 4\n    5 6 7 8\n    9 10 11 12\n    13 14 15 16)\n"
    );
    assert_eq!(fmt(&result), result, "should be idempotent");
}

#[test]
fn test_bytevector_short_stays_one_line() {
    assert_eq!(fmt("#u8(1 2 3 255)"), "#u8(1 2 3 255)\n");
}

#[test]
fn test_bytevector_with_comment_falls_back() {
    let input = "#u8(1 2 ;; c\n    3)";
    let result = fmt(input);
    assert!(result.contains(";; c"), "comment deleted:\n{result}");
    assert_eq!(fmt(&result), result, "should be idempotent");
}
