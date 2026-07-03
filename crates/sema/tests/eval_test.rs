#![allow(clippy::approx_constant)]
mod common;

use sema_core::Value;

// ============================================================
// Destructuring
// ============================================================

eval_tests! {
    // Regression: the inline ADD_INT/SUB_INT opcodes masked the result to the
    // 45-bit small-int payload with no overflow check, so a runtime add/sub whose
    // result crossed ±2^44 (~17.5 trillion) was silently truncated. Variables
    // (not literals) are required — literal operands are constant-folded and never
    // hit the runtime opcode. Found by the grammar fuzzer's distributivity law.
    big_int_add_overflows_small: "(let ((a 9000000000000)) (+ a a))" => Value::int(18000000000000),
    big_int_add_two_products: "(let ((x 10500918018048) (y 11566093991936)) (+ x y))" => Value::int(22067012009984),
    big_int_sub_overflows_small: "(let ((x 9000000000000) (y -9000000000000)) (- x y))" => Value::int(18000000000000),
    big_int_distributivity: "(let ((a 32768) (b 320462586) (c 352969177)) (- (* a (+ b c)) (+ (* a b) (* a c))))" => Value::int(0),

    // Regression: get-in must distinguish a key present with a nil value from a
    // missing key, and an empty path returns the root (found by ultracode hunt).
    // (otel/span ...) is a no-op when telemetry is disabled (no provider installed,
    // the default in tests) but still runs its thunk and returns its value.
    otel_span_disabled_returns_value: r#"(otel/span "x" (fn () (+ 40 2)))"# => Value::int(42),
    otel_event_disabled_is_noop: r#"(otel/event "tick" {:n 1})"# => Value::nil(),

    get_in_present_nil: r#"(get-in {:a nil} [:a] "default")"# => Value::nil(),
    get_in_nil_empty_path: r#"(get-in nil [] "default")"# => Value::nil(),
    get_in_missing_key: r#"(get-in {:a 1} [:b] "default")"# => Value::string("default"),
    get_in_nested: "(get-in {:a {:b 2}} [:a :b] 0)" => Value::int(2),
    // Regression: IEEE inf/-inf round-trip through printer -> reader.
    inf_round_trips: "(= (/ 1.0 0.0) (read (str (/ 1.0 0.0))))" => Value::bool(true),
    neg_inf_reads: r#"(= (/ -1.0 0.0) (read "-inf"))"# => Value::bool(true),
    // Regression: strings with escapes round-trip when printed readably (nested).
    string_newline_round_trips: r#"(= (list "a\nb") (read (str (list "a\nb"))))"# => Value::bool(true),
    string_quote_backslash_round_trips: r#"(= (list "q\"b\\s") (read (str (list "q\"b\\s"))))"# => Value::bool(true),

    destructure_let_vector: "(let (([a b] '(1 2))) (+ a b))" => Value::int(3),
    destructure_let_vector_from_vec: "(let (([a b] [10 20])) (+ a b))" => Value::int(30),
    // Hand-constructed Value to avoid eval-oracle circularity (the oracle would run the same evaluator under test)
    destructure_let_rest: "(let (([a & rest] '(1 2 3))) rest)" => Value::list(vec![Value::int(2), Value::int(3)]),
    // Hand-constructed Value to avoid eval-oracle circularity
    destructure_let_rest_empty: "(let (([a b & rest] '(1 2))) rest)" => Value::list(vec![]),
    destructure_let_wildcard: "(let (([_ b] '(1 2))) b)" => Value::int(2),
    destructure_let_nested: "(let (([[a b] c] '((1 2) 3))) (+ a b c))" => Value::int(6),
    destructure_let_map_keys: "(let (({:keys [x y]} {:x 10 :y 20})) (+ x y))" => Value::int(30),
    destructure_let_map_missing: "(let (({:keys [x y]} {:x 10})) y)" => Value::nil(),
    destructure_let_star_seq: "(let* (([a b] '(1 2)) (c (+ a b))) c)" => Value::int(3),
    destructure_define_vector: "(begin (define [a b c] '(1 2 3)) (+ a b c))" => Value::int(6),
    destructure_define_map: "(begin (define {:keys [name age]} {:name \"Alice\" :age 30}) age)" => Value::int(30),
    destructure_lambda_vector: "((lambda ([a b]) (+ a b)) '(1 2))" => Value::int(3),
    destructure_lambda_map: "((lambda ({:keys [x y]}) (+ x y)) {:x 3 :y 4})" => Value::int(7),
    destructure_lambda_mixed: "((lambda (a [b c]) (+ a b c)) 10 '(20 30))" => Value::int(60),
    destructure_nested_map_in_vec: "(let (([a {:keys [b]}] (list 1 {:b 2}))) (+ a b))" => Value::int(3),
}

eval_error_tests! {
    destructure_err_too_few: "(let (([a b c] '(1 2))) a)" => "destructure: expected 3",
    destructure_err_too_many: "(let (([a b] '(1 2 3))) a)" => "destructure: expected 2",
    destructure_err_non_list: "(let (([a b] 42)) a)" => "expected list or vector",
    destructure_err_non_map: "(let (({:keys [x]} '(1 2))) x)" => "expected map",
}

// Scientific / exponential number literals (LEX-1).
eval_tests! {
    sci_float_literal: "1.0e19" => Value::float(1e19),
    sci_bare_exponent: "1e6" => Value::float(1e6),
    sci_uppercase_e: "1E10" => Value::float(1e10),
    sci_negative_exponent: "2e-3" => Value::float(0.002),
    sci_signed_plus_exponent: "6.022e+23" => Value::float(6.022e23),
    sci_in_expression: "(* 2 3e2)" => Value::float(600.0),
    sci_int_conversion: "(int 1.5e3)" => Value::int(1500),
}

eval_error_tests! {
    // `1.0e19` parses (else the error would be "Unbound variable: e19"), then
    // overflows i64 — the error mentions `int`.
    sci_int_overflow: "(int 1.0e19)" => "int",
}

// ============================================================
// Pattern Matching
// ============================================================

eval_tests! {
    match_literal_int: r#"(match 42 (42 "found") (_ "nope"))"# => Value::string("found"),
    match_literal_string: r#"(match "hello" ("hello" 1) ("world" 2) (_ 0))"# => Value::int(1),
    match_literal_keyword: r#"(match :ok (:ok "success") (:err "failure"))"# => Value::string("success"),
    match_literal_bool: r#"(match #t (#t "yes") (#f "no"))"# => Value::string("yes"),
    match_wildcard: r#"(match 99 (1 "one") (2 "two") (_ "other"))"# => Value::string("other"),
    match_symbol_binding: "(match 42 (x (+ x 8)))" => Value::int(50),
    match_vector_pattern: "(match '(1 2 3) ([a b c] (+ a b c)))" => Value::int(6),
    // Hand-constructed Value to avoid eval-oracle circularity
    match_vector_rest: "(match '(1 2 3 4) ([a & rest] rest))" => Value::list(vec![Value::int(2), Value::int(3), Value::int(4)]),
    match_map_keys: "(match {:x 10 :y 20} ({:keys [x y]} (+ x y)))" => Value::int(30),
    match_guard: r#"(match 5 (x when (> x 10) "big") (x when (> x 0) "small") (_ "zero"))"# => Value::string("small"),
    // A wildcard present alongside guards still works under strict `match`.
    match_guard_with_wildcard: r#"(match 5 (x when (> x 100) "big") (_ "other"))"# => Value::string("other"),
    // `match*` with all guards failing and no wildcard → lenient nil.
    match_star_guards_all_fail: r#"(match* 5 (x when (> x 100) "big") (x when (< x 0) "neg"))"# => Value::nil(),
    // Strict `match` raises on no-match (see error tests); `match*` is the lenient nil form.
    match_star_no_match_nil: r#"(match* 42 (1 "one") (2 "two"))"# => Value::nil(),
    match_nested: "(match '(1 (2 3)) ([a [b c]] (+ a b c)))" => Value::int(6),
    match_nil: r#"(match nil (nil "null") (_ "other"))"# => Value::string("null"),
    match_vector_mismatch: r#"(match '(1 2 3) ([a b] "two") (_ "other"))"# => Value::string("other"),
    match_map_structural: "(match {:type :ok :val 42} ({:type :ok :val v} v) (_ nil))" => Value::int(42),
    match_map_structural_fail: r#"(match {:type :err} ({:type :ok :val v} v) (_ "fallback"))"# => Value::string("fallback"),

    // Guard + pattern failure fallthrough (regression: VM returned nil instead of trying next clause)
    match_guard_pattern_fail_fallthrough: r#"
        (match {:a 1}
          ({:a x :b y} when (> x 0) "has-both")
          ({:a x} "has-a")
          (_ "nothing"))
    "# => Value::string("has-a"),

    match_guard_pattern_fail_to_wildcard: r#"
        (match {:x 1}
          ({:x v :y w} when #t "both")
          (_ "fallback"))
    "# => Value::string("fallback"),

    match_guard_false_then_pattern_fail: r#"
        (define (ok? v) (> v 10))
        (match {:id 5}
          ({:id n} when (ok? n) "big")
          ({:id n :name s} "has-name")
          ({:id n} (+ n 100))
          (_ 0))
    "# => Value::int(105),

    match_map_guard_multi_clause: r#"
        (define (find-user id) (if (= id 1) "Alice" #f))
        (match {:method :GET :path "/users" :id 99}
          ({:method :GET :path "/users" :id id} when (find-user id)
            (find-user id))
          ({:method :GET :path "/users" :id id}
            "not-found")
          ({:method :GET :path "/users"}
            "all")
          (_ "404"))
    "# => Value::string("not-found"),

    match_map_guard_no_key_falls_to_later: r#"
        (define (find-user id) (if (= id 1) "Alice" #f))
        (match {:method :GET :path "/users"}
          ({:method :GET :path "/users" :id id} when (find-user id)
            (find-user id))
          ({:method :GET :path "/users" :id id}
            "not-found")
          ({:method :GET :path "/users"}
            "all")
          (_ "404"))
    "# => Value::string("all"),
}

// ============================================================
// Pattern Matching Edge Cases
// ============================================================

eval_tests! {
    // Guard references variables bound by nested pattern
    match_guard_nested_binding: r#"
        (match {:a [1 2]}
          ({:a [x y]} when (= (+ x y) 3) :ok)
          (_ :bad))
    "# => Value::keyword("ok"),

    // Guard must NOT run when pattern fails (3-elem pattern vs 2-elem value)
    match_guard_skipped_on_pattern_fail: r#"
        (begin
          (define c 0)
          (define (tick) (set! c (+ c 1)) #t)
          (match '(1 2)
            ([a b d] when (tick) :bad)
            (_ c)))
    "# => Value::int(0),

    // Guard runs once, returns false, falls through — side effect visible in next clause
    match_guard_runs_then_falls_through: r#"
        (begin
          (define c 0)
          (match '(1)
            ([x] when (begin (set! c (+ c 1)) #f) :no)
            ([x] c)))
    "# => Value::int(1),

    // Overlapping patterns — guards determine which fires
    match_overlapping_guards: r#"
        (match {:x 5}
          ({:x n} when (> n 10) :big)
          ({:x n} when (> n 0) :pos)
          (_ :no))
    "# => Value::keyword("pos"),

    // Empty vector matches empty list
    match_empty_vector: r#"(match '() ([] :empty) (_ :no))"# => Value::keyword("empty"),

    // Empty map matches any map
    match_empty_map: r#"(match {:x 1} ({} :any-map) (_ :no))"# => Value::keyword("any-map"),

    // Quoted symbol in match
    match_quoted_symbol: r#"(match 'hello ('hello :yes) (_ :no))"# => Value::keyword("yes"),

    // Quoted symbol doesn't match different symbol
    match_quoted_symbol_mismatch: r#"(match 'world ('hello :yes) (_ :no))"# => Value::keyword("no"),

    // Map with nested rest sequence
    // Hand-constructed Value to avoid eval-oracle circularity
    match_map_nested_rest: r#"
        (match {:xs '(1 2 3)}
          ({:xs [a & rest]} rest)
          (_ nil))
    "# => Value::list(vec![Value::int(2), Value::int(3)]),

    // All clauses fail, no wildcard — `match*` is lenient and returns nil.
    // (Strict `match` raises here; covered in the error tests.)
    match_star_all_clauses_fail: r#"
        (match* {:x [1]}
          ({:x [1 2]} :bad)
          ({:x [a b]} :bad2))
    "# => Value::nil(),

    // Nested maps
    match_nested_maps: r#"
        (match {:a {:b 42}}
          ({:a {:b v}} v)
          (_ nil))
    "# => Value::int(42),

    // Match against boolean false literal
    match_bool_false_literal: r#"
        (match #f
          (#f :false)
          (#t :true)
          (_ :other))
    "# => Value::keyword("false"),

    // Match char literal
    match_char_literal: r#"
        (match #\a
          (#\a :yes)
          (_ :no))
    "# => Value::keyword("yes"),

    // :keys binds nil for missing keys and still matches
    // Hand-constructed Value to avoid eval-oracle circularity
    match_keys_missing_binds_nil: r#"
        (match {:x 1}
          ({:keys [x y]} (list x y))
          (_ :no))
    "# => Value::list(vec![Value::int(1), Value::nil()]),

    // :keys combined with structural key check
    match_keys_with_structural: r#"
        (match {:type :ok :val 42}
          ({:type :ok :keys [val]} val)
          (_ nil))
    "# => Value::int(42),

    // Match on float
    match_float_literal: r#"(match 3.14 (3.14 :pi) (_ :no))"# => Value::keyword("pi"),

    // Match on string (already tested but including for completeness with keyword body)
    match_string_keyword_body: r#"(match "hello" ("hello" :hi) (_ :no))"# => Value::keyword("hi"),

    // First matching clause wins with guards
    match_first_clause_wins: r#"
        (match 5
          (x when (> x 10) :big)
          (x when (> x 3) :medium)
          (x :small))
    "# => Value::keyword("medium"),

    // Failed pattern's bindings don't leak between clauses
    match_no_binding_leak: r#"
        (match '(1)
          ([a b] (+ a b))
          ([x] x))
    "# => Value::int(1),
}

// ============================================================
// Regex Literals
// ============================================================

eval_tests! {
    regex_literal_basic: r#"(regex/match? #"\d+" "abc123")"# => Value::bool(true),
    regex_literal_class: r#"(regex/match? #"[a-z]+" "hello")"# => Value::bool(true),
    regex_literal_anchored: r#"(regex/match? #"^hello$" "hello")"# => Value::bool(true),
}

// ============================================================
// Host/app primitives — path safety, config dir, terminal control
// (the missing pieces for self-hosted TUI apps like Sema Coder)
// ============================================================

eval_tests! {
    // path/relative-to is pure path math (no fs).
    path_relative_to_descendant: r#"(path/relative-to "/a/b" "/a/b/c/d")"# => Value::string("c/d"),
    path_relative_to_sibling: r#"(path/relative-to "/a/b/c" "/a/x")"# => Value::string("../../x"),
    path_relative_to_same: r#"(path/relative-to "/a/b" "/a/b")"# => Value::string("."),
    // path/within? — containment after resolving `.`/`..` (lexical for non-existent paths).
    path_within_descendant: r#"(path/within? "/a/b" "/a/b/c")"# => Value::bool(true),
    path_within_self: r#"(path/within? "/a/b" "/a/b")"# => Value::bool(true),
    path_within_escape: r#"(path/within? "/a/b" "/a/x")"# => Value::bool(false),
    path_within_traversal_escape: r#"(path/within? "/a/b" "/a/b/../x")"# => Value::bool(false),
    // sys/config-dir always yields a non-empty path string.
    config_dir_is_string: "(string? (sys/config-dir))" => Value::bool(true),
    // Terminal control sequences return nil (their effect is the bytes on stdout).
    term_move_to_returns_nil: "(term/move-to 1 1)" => Value::nil(),
    term_flush_returns_nil: "(term/flush)" => Value::nil(),
    term_set_title_returns_nil: r#"(term/set-title "x")"# => Value::nil(),
    // term/strip removes full CSI/OSC sequences, not just SGR (`…m`). A cursor
    // move or OSC title must not swallow the visible text that follows it.
    term_strip_sgr: r#"(term/strip "\x1b;[31mred\x1b;[0m")"# => Value::string("red"),
    term_strip_cursor_move: r#"(term/strip "x\x1b;[10;5Hy")"# => Value::string("xy"),
    term_strip_osc_title: r#"(term/strip "\x1b;]0;title\x07;after")"# => Value::string("after"),
    term_strip_plain: r#"(term/strip "plain")"# => Value::string("plain"),
    // string/width — terminal display columns (wide chars = 2, ANSI = 0).
    string_width_ascii: r#"(string/width "hello")"# => Value::int(5),
    string_width_cjk: r#"(string/width "日本語")"# => Value::int(6),
    string_width_emoji: r#"(string/width "👋")"# => Value::int(2),
    string_width_ignores_ansi: r#"(string/width (term/rgb "hi" 1 2 3))"# => Value::int(2),
    // string/wrap — width-aware word wrapping to a list of lines.
    string_wrap_words: r#"(string/word-wrap "the quick brown fox" 10)"# => common::eval(r#"'("the quick" "brown fox")"#),
    string_wrap_hard_break: r#"(string/word-wrap "abcdefghij k" 5)"# => common::eval(r#"'("abcde" "fghij" "k")"#),
    string_wrap_keeps_newlines: r#"(string/word-wrap "a\nb" 10)"# => common::eval(r#"'("a" "b")"#),
    // Terminal setup/teardown guard macros return the body value and re-raise
    // after restoring (teardown always runs — the emitted escapes go to stdout).
    guard_alt_screen_returns_body: "(term/with-alt-screen 1 2 3)" => Value::int(3),
    guard_raw_mode_returns_body: "(io/with-raw-mode 42)" => Value::int(42),
    guard_mouse_returns_body: "(term/with-mouse 7)" => Value::int(7),
    guard_reraises_after_teardown: r#"(try (term/with-alt-screen (error "x")) (catch e "caught"))"# => Value::string("caught"),
    // string->bytevector: intuitive alias for string->utf8 (UTF-8 encode).
    string_to_bytevector_alias: r#"(bytevector->string (string->bytevector "héllo"))"# => Value::string("héllo"),
    // sema/check-string classifies a wrapped reader error as :syntax with a :span
    // (regression: the error was being wrapped, dropping the code + span).
    check_string_syntax_code: r#"(:code (car (:diagnostics (sema/check-string "(+ 1 2"))))"# => Value::string("syntax"),
    check_string_has_span: r#"(map? (:span (car (:diagnostics (sema/check-string "(+ 1 2")))))"# => Value::bool(true),
}

// ============================================================
// Agent/TUI host primitives — wave 2 (diff, secret, reflect,
// archive, markup). Process/event/git/fs need a live OS handle and
// are covered by the modules' own unit tests.
// ============================================================

eval_tests! {
    // diff round-trips: applying the unified diff reconstructs `new`.
    diff_apply_roundtrip: "(diff/apply \"a\\nb\\n\" (diff/unified \"a\\nb\\n\" \"a\\nc\\n\"))" => Value::string("a\nc\n"),
    diff_stat_added: "(:added (diff/stat (diff/unified \"a\\n\" \"a\\nb\\n\")))" => Value::int(1),
    // reflection
    read_all_count: r#"(length (read/all "(a) (b) (c)"))"# => Value::int(3),
    format_form_tidies: r#"(format/form (read/string "(define  x   1)"))"# => Value::string("(define x 1)"),
    check_string_ok: r#"(:ok (sema/check-string "(+ 1 2)"))"# => Value::bool(true),
    check_string_bad: r#"(:ok (sema/check-string "(+ 1 2"))"# => Value::bool(false),
    // secrets
    secret_redact_hides: r#"(string/contains? (secret/redact "k AKIAIOSFODNN7EXAMPLE") "redacted")"# => Value::bool(true),
    // archive: gzip is a lossless round-trip
    gzip_roundtrip_len: r#"(bytevector-length (gzip/decompress (gzip/compress "hello")))"# => Value::int(5),
    // markup
    markdown_h1: r##"(string/contains? (markdown/to-html "# Hi") "<h1>")"## => Value::bool(true),
    html_text_strips_tags: r#"(html/text "<p>Hello <b>world</b></p>")"# => Value::string("Hello world"),
    // Regressions from the wave-2 quality pass:
    // overlapping redact spans must not panic (drops the overlapping one).
    redact_spans_overlap_safe: r#"(redact/spans "0123456789" (list {:start 3 :end 6} {:start 0 :end 4}))"# => Value::string("\u{ab}redacted\u{bb}456789"),
    // diff/stat counts a removed content line that renders as "---", not as a header.
    diff_stat_content_dashes: r#"(:removed (diff/stat (diff/unified "keep\n--\n" "keep\n")))"# => Value::int(1),
}

// ============================================================
// Debug helpers
// ============================================================

eval_tests! {
    spy_returns_value: r#"(spy "test" 42)"# => Value::int(42),
    spy_returns_string: r#"(spy "tag" "hello")"# => Value::string("hello"),
    assert_true: "(assert #t)" => Value::bool(true),
    assert_truthy: "(assert 42)" => Value::bool(true),
    assert_with_msg: r#"(assert #t "ok")"# => Value::bool(true),
    assert_eq_ints: "(assert= 42 42)" => Value::bool(true),
    assert_eq_strings: r#"(assert= "hello" "hello")"# => Value::bool(true),
    time_returns_result: "(time (fn () (+ 1 2)))" => Value::int(3),
}

eval_error_tests! {
    assert_false: "(assert #f)" => "assertion failed",
    assert_nil: "(assert nil)" => "assertion failed",
    assert_with_message: r#"(assert #f "custom error")"# => "custom error",
    assert_eq_mismatch: "(assert= 1 2)" => "assertion failed",
    // Strict `match` raises when no clause matches (D3); `match*` stays lenient.
    match_no_clause_raises: r#"(match 42 (1 "one") (2 "two"))"# => "no clause matched",
    match_all_clauses_fail_raises: r#"(match {:x [1]} ({:x [1 2]} :bad) ({:x [a b c]} :bad2))"# => "no clause matched",
    // Strict `match` raises when every guard fails and there is no wildcard.
    match_guards_all_fail_raises: r#"(match 5 (x when (> x 100) "big") (x when (< x 0) "neg"))"# => "no clause matched",
}

// ============================================================
// Multimethods
// ============================================================

eval_tests! {
    // Basic dispatch on keyword
    multi_basic_dispatch: r#"
        (begin
          (defmulti area (fn (shape) (get shape :type)))
          (defmethod area :circle (fn (s) (* 3 (get s :radius) (get s :radius))))
          (defmethod area :rect (fn (s) (* (get s :width) (get s :height))))
          (area {:type :circle :radius 5}))
    "# => Value::int(75),

    multi_rect_dispatch: r#"
        (begin
          (defmulti area (fn (shape) (get shape :type)))
          (defmethod area :circle (fn (s) (* 3 (get s :radius) (get s :radius))))
          (defmethod area :rect (fn (s) (* (get s :width) (get s :height))))
          (area {:type :rect :width 4 :height 6}))
    "# => Value::int(24),

    // Default method
    multi_default_method: r#"
        (begin
          (defmulti greet (fn (x) (get x :lang)))
          (defmethod greet :en (fn (x) "hello"))
          (defmethod greet :default (fn (x) "hi"))
          (greet {:lang :fr}))
    "# => Value::string("hi"),

    // Dispatch on runtime type
    multi_type_dispatch: r#"
        (begin
          (defmulti describe (fn (x) (type x)))
          (defmethod describe :int (fn (x) "integer"))
          (defmethod describe :string (fn (x) "text"))
          (list (describe 42) (describe "hi")))
    "# => Value::list(vec![Value::string("integer"), Value::string("text")]),

    // Multi-argument dispatch
    multi_two_arg_dispatch: r#"
        (begin
          (defmulti combine (fn (a b) (list (type a) (type b))))
          (defmethod combine '(:int :int) (fn (a b) (+ a b)))
          (defmethod combine '(:string :string) (fn (a b) (string-append a b)))
          (list (combine 1 2) (combine "a" "b")))
    "# => Value::list(vec![Value::int(3), Value::string("ab")]),

    // defmethod returns nil
    multi_defmethod_returns_nil: r#"
        (begin
          (defmulti f (fn (x) x))
          (defmethod f :a (fn (x) 1)))
    "# => Value::nil(),

    // Adding methods after initial definition
    multi_open_extension: r#"
        (begin
          (defmulti op (fn (x) (get x :kind)))
          (defmethod op :add (fn (x) (+ (get x :a) (get x :b))))
          (let ((r1 (op {:kind :add :a 1 :b 2})))
            (defmethod op :mul (fn (x) (* (get x :a) (get x :b))))
            (+ r1 (op {:kind :mul :a 3 :b 4}))))
    "# => Value::int(15),

    // Dispatch on integer values
    multi_int_dispatch: r#"
        (begin
          (defmulti fizzbuzz (fn (n) (cond ((= (modulo n 15) 0) :fizzbuzz)
                                           ((= (modulo n 3) 0) :fizz)
                                           ((= (modulo n 5) 0) :buzz)
                                           (#t :num))))
          (defmethod fizzbuzz :fizzbuzz (fn (n) "FizzBuzz"))
          (defmethod fizzbuzz :fizz (fn (n) "Fizz"))
          (defmethod fizzbuzz :buzz (fn (n) "Buzz"))
          (defmethod fizzbuzz :num (fn (n) n))
          (list (fizzbuzz 15) (fizzbuzz 9) (fizzbuzz 10) (fizzbuzz 7)))
    "# => Value::list(vec![
        Value::string("FizzBuzz"),
        Value::string("Fizz"),
        Value::string("Buzz"),
        Value::int(7),
    ]),
}

// ============================================================
// String interning
// ============================================================

eval_tests! {
    string_intern_returns_string: r#"(string/intern "hello")"# => Value::string("hello"),
    string_intern_eq: r#"(equal? (string/intern "hello") (string/intern "hello"))"# => Value::bool(true),
    string_intern_same_pointer: r#"(eq? (string/intern "abc") (string/intern "abc"))"# => Value::bool(true),
    string_intern_different_strings: r#"(eq? (string/intern "a") (string/intern "b"))"# => Value::bool(false),
    string_intern_as_map_key: r#"
        (let ((k (string/intern "key")))
          (get {k 42} k))
    "# => Value::int(42),
}

eval_error_tests! {
    string_intern_wrong_type: "(string/intern 42)" => "expected string",
    string_intern_no_args: "(string/intern)" => "string/intern expects 1",
}

// ============================================================
// TOML
// ============================================================

eval_tests! {
    toml_decode_basic: r#"
        (let ((t (toml/decode "[package]\nname = \"test\"\nversion = \"1.0\"")))
          (:name (:package t)))
    "# => Value::string("test"),

    toml_decode_deps: r#"
        (let ((t (toml/decode "[deps]\nhttp = \"github.com/user/http\"")))
          (:http (:deps t)))
    "# => Value::string("github.com/user/http"),

    toml_decode_array: r#"
        (let ((t (toml/decode "tags = [\"a\", \"b\"]")))
          (length (:tags t)))
    "# => Value::int(2),

    toml_decode_nested: r#"
        (let ((t (toml/decode "[package]\nname = \"x\"\n\n[deps]\nfoo = \"bar\"")))
          (list (:name (:package t)) (:foo (:deps t))))
    "# => Value::list(vec![Value::string("x"), Value::string("bar")]),

    toml_decode_integer: r#"
        (let ((t (toml/decode "port = 8080")))
          (:port t))
    "# => Value::int(8080),

    // Approximate check: TOML parser may not preserve exact f64 bits
    toml_decode_float: r#"
        (let ((t (toml/decode "pi = 3.14")))
          (> (:pi t) 3.13))
    "# => Value::bool(true),

    toml_decode_bool: r#"
        (let ((t (toml/decode "debug = true")))
          (:debug t))
    "# => Value::bool(true),

    toml_decode_empty_table: r#"
        (let ((t (toml/decode "")))
          (map? t))
    "# => Value::bool(true),

    toml_encode_basic: r#"
        (string/contains? (toml/encode {:name "test"}) "name = \"test\"")
    "# => Value::bool(true),

    toml_roundtrip_simple: r#"
        (let* ((original {:name "sema" :version "1.0"})
               (encoded (toml/encode original))
               (decoded (toml/decode encoded)))
          (list (:name decoded) (:version decoded)))
    "# => Value::list(vec![Value::string("sema"), Value::string("1.0")]),
}

eval_error_tests! {
    toml_decode_invalid_input: r#"(toml/decode "[invalid")"# => "toml/decode",

    toml_encode_non_map: r#"(toml/encode "not a map")"# => "top-level value must be a map",

    toml_decode_wrong_type: r#"(toml/decode 42)"# => "expected string",

    toml_encode_nil_value: r#"(toml/encode {:key nil})"# => "cannot encode nil",
}

eval_error_tests! {
    // No matching method and no default
    multi_no_method: r#"
        (begin
          (defmulti f (fn (x) x))
          (defmethod f :a (fn (x) 1))
          (f :b))
    "# => "no method",
    // defmethod on non-multimethod
    multi_defmethod_not_multi: r#"
        (begin
          (define x 42)
          (defmethod x :a (fn (x) 1)))
    "# => "not a multimethod",
    // defmulti wrong arity
    multi_defmulti_no_args: "(defmulti)" => "defmulti expects 2",
    // defmethod wrong arity
    multi_defmethod_no_args: "(defmethod)" => "defmethod expects 3",
}

// Special-form names CAN be bound (the bind-site reservation was reverted in
// 1.21.2 — it broke common code like a param named `message`/`fn`). They shadow
// correctly in VALUE position; in OPERATOR/head position the special form still
// wins (a documented footgun — docs/limitations.md #36; the proper fix is full
// lexical shadowing, future work).
eval_tests! {
    shadow_if_value_position: "(let ((if 5)) (+ if 1))" => Value::int(6),
    shadow_message_value_position: r#"(let ((message "hi")) message)"# => Value::string("hi"),
    shadow_fn_as_param: "((fn (fn) (+ fn 1)) 10)" => Value::int(11),
    // Operator position: the special form wins, NOT the binding (documented limitation).
    special_form_wins_in_operator_position: "(let ((and (fn (a b) (* a b)))) (and 3 4))" => Value::int(4),
}

// Regular (non-special-form) names also shadow freely.
eval_tests! {
    shadow_builtin_list_fn: "(let ((list (fn (x) (* x 2)))) (list 5))" => Value::int(10),
    shadow_builtin_map_var: "(let ((map 7)) (+ map 1))" => Value::int(8),
}

// Actionable redirect hints on common cross-dialect mistakes. The hint lives in
// SemaError::hint() (not the Display message), so these assert on it directly.
mod redirect_hints {
    use sema_eval::Interpreter;

    fn hint_of(input: &str) -> String {
        let interp = Interpreter::new();
        let err = interp
            .eval_str_compiled(input)
            .expect_err("expected an error");
        err.hint()
            .unwrap_or_else(|| panic!("no hint on error for `{input}`: {err}"))
            .to_string()
    }

    #[test]
    fn add_mixing_string_suggests_str() {
        assert!(hint_of(r#"(+ 1 "x")"#).contains("use (str a b ...)"));
    }

    #[test]
    fn get_on_vector_suggests_nth() {
        assert!(hint_of("(get [1 2 3] 1)").contains("use (nth coll i)"));
    }

    #[test]
    fn contains_on_vector_suggests_nth() {
        assert!(hint_of("(contains? [1 2 3] 1)").contains("use (nth coll i)"));
    }

    #[test]
    fn nth_swapped_args_flagged() {
        assert!(hint_of("(nth 1 (list 10 20 30))").contains("arguments are swapped"));
    }
}

// ============================================================
// Dialect Aliases
// ============================================================

eval_tests! {
    alias_mapcar: "(mapcar (fn (x) (* x 2)) '(1 2 3))" => Value::list(vec![Value::int(2), Value::int(4), Value::int(6)]),
    alias_fold: "(fold + 0 '(1 2 3))" => Value::int(6),
    alias_every_q: "(every? odd? '(1 3 5))" => Value::bool(true),
    alias_any_q: "(any? even? '(1 2 3))" => Value::bool(true),
    alias_some_q: "(some? even? '(1 2 3))" => Value::bool(true),
    alias_string_join: r#"(string-join '("a" "b" "c") ",")"# => Value::string("a,b,c"),
    alias_string_split: r#"(string-split "a,b,c" ",")"# => Value::list(vec![Value::string("a"), Value::string("b"), Value::string("c")]),
    alias_string_trim: r#"(string-trim "  hello  ")"# => Value::string("hello"),
    alias_hash_map_q: "(hash-map? (hash-map :a 1))" => Value::bool(true),
    alias_hash_ref: "(hash-ref {:a 1 :b 2} :b)" => Value::int(2),
    alias_type_of: "(type-of 42)" => common::eval("(type 42)"),
    alias_def_simple: "(def x 42) x" => Value::int(42),
    alias_def_function: "(def (add a b) (+ a b)) (add 3 4)" => Value::int(7),
    alias_defn: "(defn add (a b) (+ a b)) (add 3 4)" => Value::int(7),
    alias_progn: "(progn (define x 10) (define y 20) (+ x y))" => Value::int(30),
}

// ============================================================
// Auto-gensym — macro hygiene tests (tree-walker + VM)
// ============================================================

eval_tests! {
    // Core hygiene: macro's x# does NOT capture user's x
    auto_gensym_basic: r#"
        (begin
          (defmacro my-let1 (val body)
            `(let ((x# ,val)) ,body))
          (let ((x 10))
            (my-let1 42 x)))
    "# => Value::int(10),

    // Same foo# within one quasiquote maps to the same gensym
    auto_gensym_consistent: r#"
        (begin
          (defmacro my-bind (val body)
            `(let ((tmp# ,val)) (+ tmp# tmp#)))
          (my-bind 21 nil))
    "# => Value::int(42),

    // Different auto-gensym names get different symbols
    auto_gensym_different_names: r#"
        (begin
          (defmacro my-bind2 (a b)
            `(let ((x# ,a) (y# ,b)) (+ x# y#)))
          (my-bind2 10 20))
    "# => Value::int(30),

    // Auto-gensym does NOT interfere with unquote
    auto_gensym_with_unquote: r#"
        (begin
          (defmacro add-one (expr)
            `(let ((tmp# ,expr)) (+ tmp# 1)))
          (add-one 41))
    "# => Value::int(42),

    // Nested macro calls get independent gensyms (no collision)
    auto_gensym_nested_calls: r#"
        (begin
          (defmacro my-inc (expr)
            `(let ((v# ,expr)) (+ v# 1)))
          (my-inc (my-inc 10)))
    "# => Value::int(12),

    // Auto-gensym symbol outside quasiquote is just a regular symbol
    auto_gensym_outside_quasiquote: r#"
        (begin
          (define x# 42)
          x#)
    "# => Value::int(42),

    // Auto-gensym works inside vectors in quasiquote
    auto_gensym_in_vector: r#"
        (begin
          (defmacro vec-bind (val)
            `(let ((v# ,val)) [v# v#]))
          (vec-bind 5))
    "# => Value::vector(vec![Value::int(5), Value::int(5)]),

    // x## (double hash) is NOT auto-gensym — only single trailing # triggers it
    auto_gensym_double_hash_is_regular: r#"
        (begin
          (define x## 99)
          x##)
    "# => Value::int(99),
}

// ============================================================
// Prelude hygiene
// ============================================================

eval_tests! {
    // some-> should not capture user's __v variable
    some_arrow_no_capture: r#"
        (begin
          (define __v {:name "Alice" :age 30})
          (some-> __v (:name)))
    "# => Value::string("Alice"),
}

// ============================================================
// Auto-gensym edge cases
// ============================================================

eval_tests! {
    // Auto-gensym with splicing
    auto_gensym_with_splicing: r#"
        (begin
          (defmacro my-do (. body)
            `(let ((r# nil)) ,@body r#))
          (my-do (define x 1) (define y 2)))
    "# => Value::nil(),

    // Multiple quasiquotes in same macro body get independent gensyms
    auto_gensym_multi_quasiquote: r#"
        (begin
          (defmacro double-bind (a b)
            (let ((first `(let ((x# ,a)) x#))
                  (second `(let ((x# ,b)) x#)))
              `(+ ,first ,second)))
          (double-bind 10 20))
    "# => Value::int(30),

    // Manual gensym and auto-gensym share a counter — no collision
    auto_gensym_no_collision_with_manual: r#"
        (begin
          (define s1 (gensym "x"))
          (defmacro my-m (v) `(let ((x# ,v)) x#))
          (my-m 42))
    "# => Value::int(42),

    // Deeply nested macro calls — each level gets its own gensyms
    auto_gensym_deep_nesting: r#"
        (begin
          (defmacro wrap (expr)
            `(let ((v# ,expr)) (+ v# 0)))
          (wrap (wrap (wrap (wrap (wrap 100))))))
    "# => Value::int(100),

    // Macro that introduces a binding with same name as user variable
    auto_gensym_shadowing_proof: r#"
        (begin
          (defmacro capture-test (body)
            `(let ((result# 999)) ,body))
          (let ((result# 1))
            (capture-test result#)))
    "# => Value::int(1),

    // Shared counter: gensym then auto-gensym produce different names
    auto_gensym_shared_counter_proof: r#"
        (begin
          (define a (gensym "x"))
          (define b `x#)
          (not (= a b)))
    "# => Value::bool(true),

    // 20-level deep nesting — no stack issues, all gensyms independent
    auto_gensym_deep_nesting_20: r#"
        (begin
          (defmacro wrap (expr)
            `(let ((v# ,expr)) (+ v# 0)))
          (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap (wrap 7)))))))))))))))))))))
    "# => Value::int(7),

    // Stress: 1000 sequential gensyms are all unique
    auto_gensym_1000_unique: r#"
        (begin
          (define syms (map (fn (_) (symbol->string (gensym "s"))) (range 1000)))
          (define sorted (sort syms))
          (define (has-dup? lst)
            (if (or (null? lst) (null? (cdr lst)))
              #f
              (if (= (car lst) (cadr lst))
                #t
                (has-dup? (cdr lst)))))
          (not (has-dup? sorted)))
    "# => Value::bool(true),

    // Stress: 100 auto-gensym macro invocations — all get unique bindings
    auto_gensym_100_macro_invocations: r#"
        (begin
          (defmacro inc-wrap (expr)
            `(let ((v# ,expr)) (+ v# 1)))
          (define (apply-n f n x)
            (if (= n 0) x (apply-n f (- n 1) (f x))))
          (apply-n (fn (x) (inc-wrap x)) 100 0))
    "# => Value::int(100),

    // Recursive macro that generates auto-gensyms at each recursion level
    auto_gensym_recursive_macro: r#"
        (begin
          (defmacro count-down (n)
            (if (= n 0)
              0
              `(let ((v# ,n)) (+ v# (count-down ,(- n 1))))))
          (count-down 10))
    "# => Value::int(55),

    // Multiple different auto-gensym names in one quasiquote all independent
    auto_gensym_many_names: r#"
        (begin
          (defmacro multi (a b c d)
            `(let ((w# ,a) (x# ,b) (y# ,c) (z# ,d))
               (+ w# x# y# z#)))
          (multi 1 2 3 4))
    "# => Value::int(10),

    // Auto-gensym in nested let bindings within one quasiquote
    auto_gensym_nested_lets: r#"
        (begin
          (defmacro nested-bind (a b)
            `(let ((outer# ,a))
               (let ((inner# ,b))
                 (+ outer# inner#))))
          (nested-bind 10 20))
    "# => Value::int(30),
}

// ============================================================
// Destructuring Edge Cases
// ============================================================

eval_tests! {
    // Deep nesting: map with nested vector value
    destructure_map_nested_vec_val: "(let (({:a [x y]} {:a [10 20]})) (+ x y))" => Value::int(30),

    // Triple nesting: vector containing map containing vector
    destructure_triple_nesting: "(let (([{:a [x]}] (list {:a [42]}))) x)" => Value::int(42),

    // Rest pattern: [& rest] binds entire sequence
    // Hand-constructed Value to avoid eval-oracle circularity
    destructure_rest_binds_all: "(let (([& rest] '(1 2 3))) rest)" => Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),

    // Nested destructure of rest: [a & [b c]]
    // Hand-constructed Value to avoid eval-oracle circularity
    destructure_nested_rest: "(let (([a & [b c]] '(1 2 3))) (list a b c))" => Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),

    // Explicit key-pattern pair in map destructuring
    destructure_map_explicit_key: "(let (({:x val} {:x 42})) val)" => Value::int(42),

    // Combined :keys + explicit key
    destructure_map_keys_and_explicit: "(let (({:keys [x] :y yval} {:x 1 :y 2})) (+ x yval))" => Value::int(3),

    // Empty map pattern binds nothing
    destructure_empty_map: "(let (({} {:x 1})) 42)" => Value::int(42),

    // Missing keys produce nil
    // Hand-constructed Value to avoid eval-oracle circularity
    destructure_map_missing_keys: "(let (({:keys [x y z]} {:x 1})) (list x y z))" => Value::list(vec![Value::int(1), Value::nil(), Value::nil()]),

    // Map destructuring from hashmap
    destructure_hashmap: "(let (({:keys [x]} (hashmap/new :x 99))) x)" => Value::int(99),

    // fn params with rest in vector destructuring
    // Hand-constructed Value to avoid eval-oracle circularity
    destructure_fn_rest: "((fn ([a & rest]) rest) '(1 2 3))" => Value::list(vec![Value::int(2), Value::int(3)]),

    // fn params with map inside vector destructuring
    destructure_fn_map_in_vec: "((fn ([{:keys [x]}]) x) (list {:x 42}))" => Value::int(42),

    // define with nested destructure
    destructure_define_nested: "(begin (define [{:keys [a]} b] (list {:a 10} 20)) (+ a b))" => Value::int(30),

    // Match with deeply nested pattern (map containing vector with rest)
    // Hand-constructed Value to avoid eval-oracle circularity
    match_deep_nested_rest: "(match {:items [1 2 3]} ({:items [a & rest]} rest) (_ nil))" => Value::list(vec![Value::int(2), Value::int(3)]),

    // Match vector exact mismatch falls through to correct clause
    match_vec_exact_fallthrough: "(match '(1 2) ([a b c] :three) ([a b] :two) (_ :other))" => Value::keyword("two"),
}

// ============================================================
// Module/function aliases
// ============================================================

eval_tests! {
    // string aliases
    string_length_alias: r#"(string/length "hello")"# => Value::int(5),
    string_append_alias: r#"(string/append "a" "b")"# => Value::string("ab"),
    string_ref_alias: r#"(string/ref "hello" 0)"# => Value::char('h'),
    string_slice_alias: r#"(string/slice "hello" 1 3)"# => Value::string("el"),
    string_to_symbol_alias: r#"(string/to-symbol "foo")"# => Value::symbol("foo"),
    symbol_to_string_alias: r#"(symbol/to-string 'foo)"# => Value::string("foo"),
    string_to_number_alias: r#"(string/to-number "42")"# => Value::int(42),
    number_to_string_alias: r#"(number/to-string 42)"# => Value::string("42"),
    string_to_keyword_alias: r#"(keyword? (string/to-keyword "foo"))"# => Value::bool(true),
    keyword_to_string_alias: r#"(keyword/to-string :foo)"# => Value::string("foo"),
    char_to_integer_alias: r#"(char/to-integer #\a)"# => Value::int(97),
    integer_to_char_alias: r#"(integer/to-char 97)"# => Value::char('a'),
    char_to_string_alias: r#"(char/to-string #\a)"# => Value::string("a"),
    string_to_char_alias: r#"(string/to-char "a")"# => Value::char('a'),
    string_to_list_alias: r#"(length (string/to-list "abc"))"# => Value::int(3),
    string_to_float_alias: r#"(string/to-float "3.14")"# => Value::float(3.14),
    char_alphabetic_alias: r#"(char/alphabetic? #\a)"# => Value::bool(true),
    char_numeric_alias: r#"(char/numeric? #\5)"# => Value::bool(true),
    char_whitespace_alias: r#"(char/whitespace? #\space)"# => Value::bool(true),
    char_upper_case_alias: r#"(char/upper-case? #\A)"# => Value::bool(true),
    char_lower_case_alias: r#"(char/lower-case? #\a)"# => Value::bool(true),
    char_upcase_alias: r#"(char/upcase #\a)"# => Value::char('A'),
    char_downcase_alias: r#"(char/downcase #\A)"# => Value::char('a'),

    // map aliases
    map_new_alias: r#"(map? (map/new :a 1))"# => Value::bool(true),
    map_deep_merge_alias: r#"(get (map/deep-merge {:a 1} {:b 2}) :b)"# => Value::int(2),
    map_get_in_alias: r#"(map/get-in {:a {:b 42}} '(:a :b))"# => Value::int(42),
    map_assoc_in_alias: r#"(map/get-in (map/assoc-in {} '(:a :b) 1) '(:a :b))"# => Value::int(1),

    // bytevector aliases
    bytevector_new_alias: r#"(bytevector/length (bytevector/new 3))"# => Value::int(3),
    bytevector_length_alias: r#"(bytevector/length (bytevector 1 2 3))"# => Value::int(3),
    bytevector_ref_alias: r#"(bytevector/ref (bytevector 10 20 30) 1)"# => Value::int(20),
    bytevector_append_alias: r#"(bytevector/length (bytevector/append (bytevector 1) (bytevector 2)))"# => Value::int(2),
    bytevector_to_list_alias: r#"(length (bytevector/to-list (bytevector 1 2 3)))"# => Value::int(3),
    string_to_utf8_alias: r#"(bytevector/length (string/to-utf8 "hi"))"# => Value::int(2),
    utf8_to_string_alias: r#"(utf8/to-string (string/to-utf8 "hello"))"# => Value::string("hello"),

    // --- streams ---
    stream_byte_buffer_is_stream: r#"(stream? (stream/byte-buffer))"# => Value::bool(true),
    stream_from_string_is_stream: r#"(stream? (stream/from-string "hello"))"# => Value::bool(true),
    stream_int_is_not_stream: r#"(stream? 42)"# => Value::bool(false),
    stream_nil_is_not_stream: r#"(stream? nil)"# => Value::bool(false),
    stream_type_name: r#"(type (stream/byte-buffer))"# => Value::keyword("stream"),
    stream_from_string_read: r#"(utf8->string (stream/read (stream/from-string "hello") 5))"# => Value::string("hello"),
    stream_from_string_read_partial: r#"(bytevector-length (stream/read (stream/from-string "hi") 10))"# => Value::int(2),
    stream_from_string_read_zero: r#"(bytevector-length (stream/read (stream/from-string "hi") 0))"# => Value::int(0),
    stream_from_string_read_byte: r#"(stream/read-byte (stream/from-string "A"))"# => Value::int(65),
    stream_from_string_read_byte_eof: r#"(let ((s (stream/from-string ""))) (stream/read-byte s))"# => Value::nil(),
    stream_byte_buffer_write_read: r#"(let ((s (stream/byte-buffer))) (stream/write s (string->utf8 "hi")) (bytevector-length (stream/to-bytes s)))"# => Value::int(2),
    stream_byte_buffer_roundtrip: r#"(let ((s (stream/byte-buffer))) (stream/write s (string->utf8 "hello")) (utf8->string (stream/to-bytes s)))"# => Value::string("hello"),
    stream_write_byte: r#"(let ((s (stream/byte-buffer))) (stream/write-byte s 65) (stream/write-byte s 66) (utf8->string (stream/to-bytes s)))"# => Value::string("AB"),
    stream_write_returns_count: r#"(let ((s (stream/byte-buffer))) (stream/write s (bytevector 1 2 3)))"# => Value::int(3),
    stream_readable_true: r#"(stream/readable? (stream/from-string "x"))"# => Value::bool(true),
    stream_writable_false: r#"(stream/writable? (stream/from-string "x"))"# => Value::bool(false),
    stream_writable_true: r#"(stream/writable? (stream/byte-buffer))"# => Value::bool(true),
    stream_readable_buffer: r#"(stream/readable? (stream/byte-buffer))"# => Value::bool(true),
    stream_available_true: r#"(stream/available? (stream/from-string "x"))"# => Value::bool(true),
    stream_available_false: r#"(stream/available? (stream/from-string ""))"# => Value::bool(false),
    stream_close_returns_nil: r#"(stream/close (stream/from-string "x"))"# => Value::nil(),
    stream_double_close_ok: r#"(let ((s (stream/from-string "x"))) (stream/close s) (stream/close s))"# => Value::nil(),
    stream_type_byte_buffer: r#"(stream/type (stream/byte-buffer))"# => Value::string("byte-buffer"),
    stream_type_string: r#"(stream/type (stream/from-string "x"))"# => Value::string("string"),
    stream_from_bytes_read: r#"(stream/read-byte (stream/from-bytes (bytevector 42)))"# => Value::int(42),
    stream_from_bytes_eof: r#"(let ((s (stream/from-bytes (bytevector)))) (stream/read-byte s))"# => Value::nil(),
    stream_flush_noop: r#"(stream/flush (stream/byte-buffer))"# => Value::nil(),
    stream_write_byte_nil: r#"(stream/write-byte (stream/byte-buffer) 0)"# => Value::nil(),
    stream_sequential_reads: r#"(let ((s (stream/from-string "abc"))) (stream/read-byte s) (stream/read-byte s))"# => Value::int(98),
    stream_to_string: r#"(let ((s (stream/byte-buffer))) (stream/write s (string->utf8 "ok")) (stream/to-string s))"# => Value::string("ok"),
    stream_identity_eq: r#"(let ((s (stream/byte-buffer))) (eq? s s))"# => Value::bool(true),

    // stream/read-line on in-memory streams
    stream_read_line_basic: r#"(stream/read-line (stream/from-string "hello\nworld"))"# => Value::string("hello"),
    stream_read_line_second: r#"(let ((s (stream/from-string "a\nb"))) (stream/read-line s) (stream/read-line s))"# => Value::string("b"),
    stream_read_line_no_newline: r#"(stream/read-line (stream/from-string "abc"))"# => Value::string("abc"),
    stream_read_line_empty: r#"(stream/read-line (stream/from-string ""))"# => Value::nil(),
    stream_read_line_crlf: r#"(stream/read-line (stream/from-string "hello\r\nworld"))"# => Value::string("hello"),
    stream_read_line_only_newline: r#"(stream/read-line (stream/from-string "\n"))"# => Value::string(""),

    // with-stream actually closes
    with_stream_read_after_close: r#"(let ((outer nil))
        (with-stream (s (stream/from-string "x")) (set! outer s))
        (try (stream/read outer 1) (catch e :closed)))"# => Value::keyword("closed"),

    // stream/flush on closed stream errors
    stream_flush_closed_errors: r#"(try (let ((s (stream/byte-buffer))) (stream/close s) (stream/flush s)) (catch e :error))"# => Value::keyword("error"),

    // stream/read-all on closed stream errors
    stream_read_all_closed: r#"(try (let ((s (stream/from-string "x"))) (stream/close s) (stream/read-all s)) (catch e :error))"# => Value::keyword("error"),

    // zero-length write returns 0
    stream_write_empty: r#"(stream/write (stream/byte-buffer) (bytevector))"# => Value::int(0),

    // --- with-stream macro ---
    with_stream_basic: r#"(with-stream (s (stream/from-string "hello")) (utf8->string (stream/read-all s)))"# => Value::string("hello"),
    with_stream_returns_body: r#"(with-stream (s (stream/byte-buffer)) (stream/write s (bytevector 1 2 3)) 42)"# => Value::int(42),
    with_stream_closes: r#"(let ((outer nil)) (with-stream (s (stream/from-string "x")) (set! outer s)) (stream? outer))"# => Value::bool(true),

    // --- PIO instruction builders ---
    pio_nop_op: r#"(get (pio/nop) :op)"# => Value::keyword("mov"),
    pio_nop_dest: r#"(get (pio/nop) :dest)"# => Value::keyword("y"),
    pio_nop_source: r#"(get (pio/nop) :source)"# => Value::keyword("y"),
    pio_jmp_target: r#"(get (pio/jmp 'foo) :target)"# => Value::symbol("foo"),
    pio_jmp_default_cond: r#"(get (pio/jmp 'foo) :cond)"# => Value::keyword("always"),
    pio_jmp_with_cond: r#"(get (pio/jmp :!x 'bar) :cond)"# => Value::keyword("!x"),
    pio_set_value: r#"(get (pio/set :pins 1) :value)"# => Value::int(1),
    pio_set_dest: r#"(get (pio/set :x 31) :dest)"# => Value::keyword("x"),
    pio_out_bits: r#"(get (pio/out :x 32) :bits)"# => Value::int(32),
    pio_in_source: r#"(get (pio/in :pins 8) :source)"# => Value::keyword("pins"),
    pio_push_defaults: r#"(get (pio/push) :block)"# => Value::bool(true),
    pio_pull_ifempty: r#"(get (pio/pull :ifempty) :ifempty)"# => Value::bool(true),
    pio_mov_dest: r#"(get (pio/mov :x :y) :dest)"# => Value::keyword("x"),
    pio_mov_invert: r#"(get (pio/mov :x :!y) :source)"# => Value::keyword("!y"),
    pio_mov_reverse: r#"(get (pio/mov :x :y :reverse) :mov-op)"# => Value::keyword("reverse"),
    pio_irq_mode: r#"(get (pio/irq :wait 3) :mode)"# => Value::keyword("wait"),
    pio_wait_polarity: r#"(get (pio/wait 1 :gpio 5) :polarity)"# => Value::int(1),
    pio_side_wraps: r#"(get (pio/side 1 (pio/nop)) :side-set)"# => Value::int(1),
    pio_delay_wraps: r#"(get (pio/delay 7 (pio/nop)) :delay)"# => Value::int(7),
    pio_side_delay_compose: r#"(get (pio/side 1 (pio/delay 3 (pio/nop))) :side-set)"# => Value::int(1),
    pio_delay_side_compose: r#"(get (pio/delay 3 (pio/side 1 (pio/nop))) :delay)"# => Value::int(3),

    // --- PIO assembly: single instructions ---
    pio_asm_nop: r#"(get (pio/assemble (list (pio/nop))) :instructions)"# =>
        Value::bytevector(vec![0x42, 0xA0]),
    pio_asm_set_pins_1: r#"(get (pio/assemble (list (pio/set :pins 1))) :instructions)"# =>
        Value::bytevector(vec![0x01, 0xE0]),
    pio_asm_set_x_31: r#"(get (pio/assemble (list (pio/set :x 31))) :instructions)"# =>
        Value::bytevector(vec![0x3F, 0xE0]),
    pio_asm_out_pins_1: r#"(get (pio/assemble (list (pio/out :pins 1))) :instructions)"# =>
        Value::bytevector(vec![0x01, 0x60]),
    pio_asm_out_x_32: r#"(get (pio/assemble (list (pio/out :x 32))) :instructions)"# =>
        Value::bytevector(vec![0x20, 0x60]),
    pio_asm_in_pins_8: r#"(get (pio/assemble (list (pio/in :pins 8))) :instructions)"# =>
        Value::bytevector(vec![0x08, 0x40]),
    pio_asm_push_block: r#"(get (pio/assemble (list (pio/push))) :instructions)"# =>
        Value::bytevector(vec![0x20, 0x80]),
    pio_asm_pull_block: r#"(get (pio/assemble (list (pio/pull))) :instructions)"# =>
        Value::bytevector(vec![0xA0, 0x80]),
    pio_asm_mov_x_y: r#"(get (pio/assemble (list (pio/mov :x :y))) :instructions)"# =>
        Value::bytevector(vec![0x22, 0xA0]),
    pio_asm_mov_x_invert_y: r#"(get (pio/assemble (list (pio/mov :x :!y))) :instructions)"# =>
        Value::bytevector(vec![0x2A, 0xA0]),
    pio_asm_wait_gpio_15: r#"(get (pio/assemble (list (pio/wait 1 :gpio 15))) :instructions)"# =>
        Value::bytevector(vec![0x8F, 0x20]),
    pio_asm_irq_set_0: r#"(get (pio/assemble (list (pio/irq :set 0))) :instructions)"# =>
        Value::bytevector(vec![0x00, 0xC0]),
    pio_asm_set_with_delay: r#"(get (pio/assemble (list (pio/delay 3 (pio/set :pins 1)))) :instructions)"# =>
        Value::bytevector(vec![0x01, 0xE3]),
    pio_asm_nop_delay_31: r#"(get (pio/assemble (list (pio/delay 31 (pio/nop)))) :instructions)"# =>
        Value::bytevector(vec![0x42, 0xBF]),

    // --- PIO assembly: labels ---
    pio_asm_jmp_backward: r#"(get (pio/assemble (list 'loop (pio/nop) (pio/jmp 'loop))) :instructions)"# =>
        Value::bytevector(vec![0x42, 0xA0, 0x00, 0x00]),
    pio_asm_jmp_forward: r#"(get (pio/assemble (list (pio/jmp 'end) (pio/nop) 'end (pio/nop))) :instructions)"# =>
        Value::bytevector(vec![0x02, 0x00, 0x42, 0xA0, 0x42, 0xA0]),
    pio_asm_jmp_cond_label: r#"(get (pio/assemble (list 'x (pio/jmp :!x 'x))) :instructions)"# =>
        Value::bytevector(vec![0x20, 0x00]),
    pio_asm_length: r#"(get (pio/assemble (list (pio/nop) (pio/nop) (pio/nop))) :length)"# =>
        Value::int(3),

    // --- PIO assembly: wrap points ---
    pio_asm_wrap_target: r#"(get (pio/assemble (list (pio/nop) :wrap-target (pio/nop) (pio/nop) :wrap)) :wrap-target)"# =>
        Value::int(1),
    pio_asm_wrap: r#"(get (pio/assemble (list (pio/nop) :wrap-target (pio/nop) (pio/nop) :wrap)) :wrap)"# =>
        Value::int(2),
    pio_asm_default_wrap_target: r#"(get (pio/assemble (list (pio/nop))) :wrap-target)"# =>
        Value::int(0),
    pio_asm_default_wrap: r#"(get (pio/assemble (list (pio/nop) (pio/nop))) :wrap)"# =>
        Value::int(1),

    // --- PIO assembly: side-set config ---
    pio_asm_side_set_config: r#"(get (pio/assemble (list (pio/side 1 (pio/set :pins 0))) {:side-set-bits 1}) :instructions)"# =>
        Value::bytevector(vec![0x00, 0xF0]),

    // --- PIO assembly: additional instruction variants ---
    pio_asm_push_iffull: r#"(get (pio/assemble (list (pio/push :iffull))) :instructions)"# =>
        Value::bytevector(vec![0x60, 0x80]),  // bit7=0,iffull=1,block=1: 0b0_1_1_00000=0x60
    pio_asm_push_noblock: r#"(get (pio/assemble (list (pio/push :no-block))) :instructions)"# =>
        Value::bytevector(vec![0x00, 0x80]),  // bit7=0,iffull=0,block=0: 0x00
    pio_asm_pull_ifempty_noblock: r#"(get (pio/assemble (list (pio/pull :ifempty :no-block))) :instructions)"# =>
        Value::bytevector(vec![0xC0, 0x80]),  // bit7=1,ifempty=1,block=0: 0b1_1_0_00000=0xC0
    pio_asm_mov_reverse: r#"(get (pio/assemble (list (pio/mov :x :y :reverse))) :instructions)"# =>
        Value::bytevector(vec![0x32, 0xA0]),  // dest=x=1,op=reverse=2,src=y=2: (1<<5)|(2<<3)|2=0x32
    pio_asm_irq_wait_rel: r#"(get (pio/assemble (list (pio/irq :wait 0 :rel))) :instructions)"# =>
        Value::bytevector(vec![0x30, 0xC0]),  // mode=wait=0b01,rel=1,index=0: (1<<5)|(1<<4)|0=0x30
    pio_asm_irq_clear: r#"(get (pio/assemble (list (pio/irq :clear 3))) :instructions)"# =>
        Value::bytevector(vec![0x43, 0xC0]),  // mode=clear=0b10,rel=0,index=3: (2<<5)|3=0x43
    pio_asm_wait_irq_rel: r#"(get (pio/assemble (list (pio/wait 1 :irq 2 :rel))) :instructions)"# =>
        Value::bytevector(vec![0xD2, 0x20]),  // pol=1,src=irq=2,rel|idx=0x10|2=0x12: (1<<7)|(2<<5)|0x12=0xD2
    pio_asm_jmp_x_dec: r#"(get (pio/assemble (list 'x (pio/jmp :x-- 'x))) :instructions)"# =>
        Value::bytevector(vec![0x40, 0x00]),  // cond=x--=2,addr=0: (2<<5)|0=0x40
    pio_asm_jmp_y_dec: r#"(get (pio/assemble (list 'y (pio/jmp :y-- 'y))) :instructions)"# =>
        Value::bytevector(vec![0x80, 0x00]),  // cond=y--=4,addr=0: (4<<5)|0=0x80
    pio_asm_jmp_osre: r#"(get (pio/assemble (list 'x (pio/jmp :!osre 'x))) :instructions)"# =>
        Value::bytevector(vec![0xE0, 0x00]),  // cond=!osre=7,addr=0: (7<<5)|0=0xE0
    pio_asm_in_osr_32: r#"(get (pio/assemble (list (pio/in :osr 32))) :instructions)"# =>
        Value::bytevector(vec![0xE0, 0x40]),  // src=osr=7,bits=32->0: (7<<5)|0=0xE0

    // --- PIO assembly: real programs ---
    pio_asm_blink: r#"(get (pio/assemble (list
        :wrap-target
        (pio/set :pins 1)
        (pio/delay 31 (pio/nop))
        (pio/set :pins 0)
        (pio/delay 31 (pio/nop))
        :wrap)) :length)"# => Value::int(4),

    // --- PIO assembly: reference test vector (hello.pio from pico-examples) ---
    // hello.pio: pull block (0x80A0), out pins 1 (0x6001), jmp 0 (0x0000)
    pio_asm_hello_reference: r#"(get (pio/assemble (list
        'start
        (pio/pull)
        (pio/out :pins 1)
        (pio/jmp 'start))) :instructions)"# =>
        Value::bytevector(vec![0xA0, 0x80, 0x01, 0x60, 0x00, 0x00]),
}

eval_error_tests! {
    // & without rest pattern name
    destructure_err_amp_no_rest: "(let (([a &] '(1 2))) a)" => "`&` must be followed by a rest pattern",

    // Multiple patterns after &
    destructure_err_amp_multiple: "(let (([a & b c] '(1 2 3))) a)" => "only one pattern allowed after `&`",

    // Non-map value for map destructure
    destructure_err_non_map_int: "(let (({:keys [x]} 42)) x)" => "expected map",

    // Nested destructure on nil value (key missing → nil, can't destructure nil as vector)
    destructure_err_nested_nil: "(let (({:a [x y]} {})) x)" => "expected list or vector",

    // --- stream errors ---
    stream_read_wrong_type: "(stream/read 42 5)" => "expected stream",
    stream_write_wrong_type: "(stream/write 42 (bytevector 1))" => "expected stream",
    stream_write_to_readonly: r#"(stream/write (stream/from-string "x") (bytevector 1))"# => "read-only",
    stream_read_closed: r#"(let ((s (stream/from-string "hi"))) (stream/close s) (stream/read s 1))"# => "stream is closed",
    stream_write_closed: r#"(let ((s (stream/byte-buffer))) (stream/close s) (stream/write s (bytevector 1)))"# => "stream is closed",
    stream_read_byte_wrong_type: "(stream/read-byte 42)" => "expected stream",
    stream_write_byte_range: "(let ((s (stream/byte-buffer))) (stream/write-byte s 256))" => "out of range",
    stream_write_byte_negative: "(let ((s (stream/byte-buffer))) (stream/write-byte s -1))" => "out of range",
    stream_to_bytes_wrong_stream: r#"(stream/to-bytes (stream/from-string "x"))"# => "expected byte-buffer stream",
    stream_to_string_wrong_stream: r#"(stream/to-string (stream/from-string "x"))"# => "expected byte-buffer stream",
    stream_read_negative_count: "(stream/read (stream/from-string \"x\") -1)" => "non-negative",
    stream_from_string_wrong_type: "(stream/from-string 42)" => "expected string",
    stream_from_bytes_wrong_type: "(stream/from-bytes 42)" => "expected bytevector",

    // --- PIO errors ---
    pio_err_undefined_label: r#"(pio/assemble (list (pio/jmp 'nowhere)))"# => "undefined label",
    pio_err_duplicate_label: r#"(pio/assemble (list 'x (pio/nop) 'x (pio/nop)))"# => "duplicate label",
    pio_err_set_value_too_large: "(pio/set :pins 32)" => "out of range",
    pio_err_set_invalid_dest: "(pio/set :foo 1)" => "unknown destination",
    pio_err_jmp_target_not_symbol: "(pio/jmp 42)" => "expected symbol",
    pio_err_delay_too_large: "(pio/delay 32 (pio/nop))" => "out of range",
    pio_err_invalid_jmp_cond: "(pio/jmp :bogus 'x)" => "unknown condition",
    pio_err_bit_count_zero: "(pio/in :pins 0)" => "bit count",
    pio_err_bit_count_33: "(pio/out :pins 33)" => "bit count",
    pio_err_invalid_in_source: "(pio/in :foo 8)" => "unknown source",
    pio_err_invalid_out_dest: "(pio/out :foo 8)" => "unknown destination",
    pio_err_invalid_mov_dest: "(pio/mov :foo :y)" => "unknown destination",
    pio_err_invalid_mov_source: "(pio/mov :x :foo)" => "unknown source",
    pio_err_irq_index_too_large: "(pio/irq :set 8)" => "out of range",
    pio_err_wait_polarity: "(pio/wait 2 :gpio 0)" => "polarity must be 0 or 1",
    pio_err_set_negative: "(pio/set :pins -1)" => "out of range",
    pio_err_pull_bad_option: "(pio/pull :bogus)" => "unexpected option",
    pio_err_push_bad_option: "(pio/push :bogus)" => "unexpected option",
    pio_err_mov_bad_operation: "(pio/mov :x :y :bogus)" => "unknown operation",
    pio_err_irq_bad_mode: "(pio/irq :bogus 0)" => "unknown mode",
    pio_err_wait_bad_source: "(pio/wait 1 :bogus 0)" => "unknown source",
}

// ============================================================
// Typed Arrays
// ============================================================

eval_tests! {
    // f64-array: make + ref
    f64_array_make_and_ref: "(f64-array/ref (f64-array/make 3 1.5) 0)" => Value::float(1.5),
    f64_array_make_default_fill: "(f64-array/ref (f64-array/make 3) 1)" => Value::float(0.0),

    // f64-array: from-list + sum
    f64_array_from_list_sum: "(f64-array/sum (f64-array/from-list '(1.0 2.0 3.0)))" => Value::float(6.0),

    // f64-array: length
    f64_array_length: "(f64-array/length (f64-array/make 5))" => Value::int(5),
    f64_array_length_from_list: "(f64-array/length (f64-array/from-list '(10 20 30)))" => Value::int(3),

    // f64-array: dot product
    f64_array_dot: "(f64-array/dot (f64-array/from-list '(1.0 2.0 3.0)) (f64-array/from-list '(4.0 5.0 6.0)))" => Value::float(32.0),

    // f64-array: map
    f64_array_map: "(f64-array/sum (f64-array/map (fn (x) (* x 2.0)) (f64-array/from-list '(1.0 2.0 3.0))))" => Value::float(12.0),

    // f64-array: fold
    f64_array_fold: "(f64-array/fold (fn (acc x) (+ acc x)) 0.0 (f64-array/from-list '(1.0 2.0 3.0 4.0)))" => Value::float(10.0),

    // f64-array: range
    f64_array_range: "(f64-array/length (f64-array/range 0 5))" => Value::int(5),
    f64_array_range_sum: "(f64-array/sum (f64-array/range 1 4))" => Value::float(6.0),

    // i64-array: make + ref
    i64_array_make_and_ref: "(i64-array/ref (i64-array/make 3 7) 2)" => Value::int(7),
    i64_array_make_default_fill: "(i64-array/ref (i64-array/make 4) 0)" => Value::int(0),

    // i64-array: from-list + sum
    i64_array_from_list_sum: "(i64-array/sum (i64-array/from-list '(10 20 30)))" => Value::int(60),

    // i64-array: map (squares 1..4 → sum is 1+4+9+16 = 30)
    i64_array_map_squares_sum: "(i64-array/sum (i64-array/map (fn (x) (* x x)) (i64-array 1 2 3 4)))" => Value::int(30),
    i64_array_map_squares_len: "(i64-array/length (i64-array/map (fn (x) (* x x)) (i64-array 1 2 3 4)))" => Value::int(4),
    i64_array_map_squares_first: "(i64-array/ref (i64-array/map (fn (x) (* x x)) (i64-array 1 2 3 4)) 0)" => Value::int(1),
    i64_array_map_squares_last: "(i64-array/ref (i64-array/map (fn (x) (* x x)) (i64-array 1 2 3 4)) 3)" => Value::int(16),
    i64_array_map_empty: "(i64-array/length (i64-array/map (fn (x) (* x x)) (i64-array)))" => Value::int(0),

    // i64-array: fold
    i64_array_fold_sum: "(i64-array/fold + 0 (i64-array 1 2 3))" => Value::int(6),
    i64_array_fold_empty_returns_init: "(i64-array/fold + 42 (i64-array))" => Value::int(42),
    i64_array_fold_mul: "(i64-array/fold (fn (acc x) (* acc x)) 1 (i64-array 2 3 4))" => Value::int(24),

    // i64-array: range — 2-arg form
    i64_array_range_2arg_len: "(i64-array/length (i64-array/range 0 5))" => Value::int(5),
    i64_array_range_2arg_first: "(i64-array/ref (i64-array/range 0 5) 0)" => Value::int(0),
    i64_array_range_2arg_last: "(i64-array/ref (i64-array/range 0 5) 4)" => Value::int(4),
    i64_array_range_2arg_sum: "(i64-array/sum (i64-array/range 0 5))" => Value::int(10),

    // i64-array: range — 3-arg form with step
    i64_array_range_step2_len: "(i64-array/length (i64-array/range 0 10 2))" => Value::int(5),
    i64_array_range_step2_first: "(i64-array/ref (i64-array/range 0 10 2) 0)" => Value::int(0),
    i64_array_range_step2_last: "(i64-array/ref (i64-array/range 0 10 2) 4)" => Value::int(8),
    i64_array_range_step2_sum: "(i64-array/sum (i64-array/range 0 10 2))" => Value::int(20),

    // i64-array/range: start > end with positive step → empty
    i64_array_range_start_gt_end_empty: "(i64-array/length (i64-array/range 5 0))" => Value::int(0),

    // i64-array/range: negative step counts down
    i64_array_range_negative_step_len: "(i64-array/length (i64-array/range 5 0 -1))" => Value::int(5),
    i64_array_range_negative_step_first: "(i64-array/ref (i64-array/range 5 0 -1) 0)" => Value::int(5),
    i64_array_range_negative_step_last: "(i64-array/ref (i64-array/range 5 0 -1) 4)" => Value::int(1),
    // Negative step with start < end → empty
    i64_array_range_negative_step_empty: "(i64-array/length (i64-array/range 0 5 -1))" => Value::int(0),

    // i64-array/set!: in-bounds write observed via ref
    i64_array_set_in_bounds_ref: "(i64-array/ref (i64-array/set! (i64-array 10 20 30) 1 99) 1)" => Value::int(99),
    i64_array_set_other_indices_unchanged: "(i64-array/ref (i64-array/set! (i64-array 10 20 30) 1 99) 0)" => Value::int(10),
    i64_array_set_last: "(i64-array/ref (i64-array/set! (i64-array 10 20 30) 2 7) 2)" => Value::int(7),

    // f64-array/set!: in-bounds write observed via ref
    f64_array_set_in_bounds_ref: "(f64-array/ref (f64-array/set! (f64-array 1.0 2.0 3.0) 1 9.5) 1)" => Value::float(9.5),
    f64_array_set_other_indices_unchanged: "(f64-array/ref (f64-array/set! (f64-array 1.0 2.0 3.0) 1 9.5) 0)" => Value::float(1.0),
    // f64-array/set! accepts int and coerces to float
    f64_array_set_accepts_int: "(f64-array/ref (f64-array/set! (f64-array 1.0 2.0 3.0) 0 42) 0)" => Value::float(42.0),

    // type predicates
    f64_array_predicate_true: "(f64-array? (f64-array/make 1))" => Value::bool(true),
    f64_array_predicate_false: "(f64-array? 42)" => Value::bool(false),
    i64_array_predicate_true: "(i64-array? (i64-array/make 1))" => Value::bool(true),
    i64_array_predicate_false: "(i64-array? \"hello\")" => Value::bool(false),
    f64_array_not_i64: "(i64-array? (f64-array/make 1))" => Value::bool(false),
    i64_array_not_f64: "(f64-array? (i64-array/make 1))" => Value::bool(false),
}

eval_error_tests! {
    // i64-array/map: callback must return integer
    i64_array_map_callback_non_int: r#"(i64-array/map (fn (x) "oops") (i64-array 1 2 3))"#,
    // i64-array/fold: wrong arity (no array argument)
    i64_array_fold_arity_too_few: "(i64-array/fold + 0)",
    i64_array_fold_arity_too_many: "(i64-array/fold + 0 (i64-array 1 2) 99)",

    // i64-array/range: zero step is an error
    i64_array_range_step_zero: "(i64-array/range 0 5 0)",

    // i64-array/set!: out-of-bounds
    i64_array_set_out_of_bounds: "(i64-array/set! (i64-array 1 2 3) 5 99)",
    // i64-array/set!: wrong value type
    i64_array_set_wrong_type: r#"(i64-array/set! (i64-array 1 2 3) 0 "oops")"#,

    // f64-array/set!: out-of-bounds
    f64_array_set_out_of_bounds: "(f64-array/set! (f64-array 1.0 2.0 3.0) 5 9.5)",
    // f64-array/set!: wrong value type (string into f64-array)
    f64_array_set_wrong_type: r#"(f64-array/set! (f64-array 1.0 2.0 3.0) 0 "oops")"#,
}

// ============================================================
// procedure? / fn?
// ============================================================

eval_tests! {
    procedure_pred_native_fn: "(procedure? +)" => Value::bool(true),
    procedure_pred_lambda: "(procedure? (fn (x) x))" => Value::bool(true),
    procedure_pred_int: "(procedure? 1)" => Value::bool(false),
    procedure_pred_string: r#"(procedure? "abc")"# => Value::bool(false),
    procedure_pred_nil: "(procedure? nil)" => Value::bool(false),
    procedure_pred_list: "(procedure? '(1 2 3))" => Value::bool(false),
    // fn? alias should behave identically
    fn_pred_native_fn: "(fn? +)" => Value::bool(true),
    fn_pred_lambda: "(fn? (fn (x) x))" => Value::bool(true),
    fn_pred_int: "(fn? 42)" => Value::bool(false),
}

// ============================================================
// reverse and filter
// ============================================================

eval_tests! {
    // reverse
    reverse_basic: "(reverse '(1 2 3))" => Value::list(vec![Value::int(3), Value::int(2), Value::int(1)]),
    reverse_empty: "(reverse '())" => Value::list(vec![]),

    // filter
    filter_even: "(filter even? '(1 2 3 4 5 6))" => Value::list(vec![Value::int(2), Value::int(4), Value::int(6)]),
    filter_none_match: "(filter even? '(1 3 5))" => Value::list(vec![]),
    filter_all_match: "(filter odd? '(1 3 5))" => Value::list(vec![Value::int(1), Value::int(3), Value::int(5)]),
}

// ============================================================
// Input validation — negative counts/indices (C7, C8, C9)
// ============================================================

eval_error_tests! {
    string_repeat_negative_errors: r#"(string/repeat "ab" -1)"# => "non-negative",
    abs_i64_min_errors: "(abs -9223372036854775808)" => "overflows i64",
    // TODO(test-strength): VM `nth` uses generic "out of bounds" while tree-walker
    // says "non-negative" — strengthen after error UX wave unifies them.
    nth_negative_errors: "(nth (list 1 2 3) -1)",
    take_negative_errors: "(take -1 (list 1 2 3))" => "non-negative",
    drop_negative_errors: "(drop -1 (list 1 2 3))" => "non-negative",
    force_non_promise_errors: "(force 42)" => "thunk",
}

// Integer arithmetic is intentionally wrapping; pin current semantics so a
// future regression away from wrap is loud.
eval_tests! {
    add_overflow_wraps: "(+ 9223372036854775807 1)" => Value::int(i64::MIN),
    sub_underflow_wraps: "(- -9223372036854775808 1)" => Value::int(i64::MAX),
}

// ============================================================
// Naming aliases — canonical slash/predicate-? names (Decision #24)
// ============================================================
// These tests guard against accidental removal of a canonical alias for an
// existing legacy stdlib function. They spot-check that the alias is bound
// and dispatches to the same implementation as the legacy name.

eval_tests! {
    alias_any_question_true: "(any? odd? (list 1 2 3))" => Value::bool(true),
    alias_any_question_false: "(any? odd? (list 2 4 6))" => Value::bool(false),
    alias_every_question_true: "(every? odd? (list 1 3 5))" => Value::bool(true),
    alias_every_question_false: "(every? odd? (list 1 2 3))" => Value::bool(false),
    alias_map_new_basic: "(get (map/new :a 1 :b 2) :b)" => Value::int(2),
    alias_async_forced_unforced: "(async/forced? (delay 1))" => Value::bool(false),
    alias_async_forced_after_force: "(let ((p (delay 1))) (force p) (async/forced? p))" => Value::bool(true),
    alias_bytevector_make: "(bytevector/length (bytevector/make 4 7))" => Value::int(4),
    alias_bytevector_u8_ref: "(bytevector/u8-ref (bytevector 10 20 30) 1)" => Value::int(20),
    alias_bytevector_u8_set: "(bytevector/u8-ref (bytevector/u8-set! (bytevector 0 0 0) 1 9) 1)" => Value::int(9),
    alias_bytevector_from_list: "(bytevector/length (bytevector/from-list (list 1 2 3 4 5)))" => Value::int(5),
    alias_bytevector_to_list: "(bytevector/to-list (bytevector 1 2 3))" => Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
    alias_bytevector_length: "(bytevector/length (bytevector 1 2 3 4))" => Value::int(4),
}

/// `time/now-ms` is non-deterministic, so we can't use the eval_tests! macro.
/// Just confirm the alias binds and returns a positive int.
#[test]
fn alias_time_now_ms() {
    let v = common::eval("(time/now-ms)");
    assert!(
        v.as_int().is_some(),
        "time/now-ms should return int, got {v:?}"
    );
    assert!(v.as_int().unwrap() > 0);
}

// ============================================================
// Audit regressions — IGNORED until upvalue model lands
// ============================================================
// These tests document known bugs in the VM backend. They assert the *correct*
// behavior (matching the tree-walker), so once the open-upvalue runtime is in
// place and these tests are un-ignored they will act as confirmation that the
// fix landed.
//
// See docs/limitations.md #31 (C1) and docs/adr.md ADR #55.

/// C1 (FIXED 2026-06-18): `set!` on a let-bound variable from a closure called
/// via a stdlib HOF (here `map`) used to be silently lost on the VM backend due
/// to the eager-close upvalue model + fresh-VM fallback. HOF callbacks are now
/// routed back into the running VM so the open upvalue cells stay connected.
/// See docs/plans/2026-06-18-c1-vm-hof-in-vm.md.
///
/// Reproduction (from the audit):
///   sema -e '(let ((c 0)) (map (fn (x) (set! c (+ c x))) (list 1 2 3)) c)'  -> 6
#[test]
fn vm_set_through_map_hof_propagates() {
    let src = "(let ((c 0)) (map (fn (x) (set! c (+ c x))) (list 1 2 3)) c)";
    assert_eq!(
        common::eval(src),
        Value::int(6),
        "set! through a map HOF callback must propagate (C1)"
    );
}

/// C1 (FIXED): same issue surfaced with `for-each`.
#[test]
fn vm_set_through_for_each_hof_propagates() {
    let src = "(let ((c 0)) (for-each (fn (x) (set! c (+ c x))) (list 1 2 3)) c)";
    assert_eq!(
        common::eval(src),
        Value::int(6),
        "set! through a for-each HOF callback must propagate (C1)"
    );
}

/// C1 related: `(type (fn (x) x))` should be `:lambda` on both backends.
/// VM currently returns `:native-fn` because closures are wrapped as NativeFn
/// for stdlib HOF interop (Decision #50). Once the open-upvalue model removes
/// the cross-VM-copy hack, this should unify.
#[test]
#[ignore = "C1 related: VM type reflection — see docs/limitations.md #31"]
fn vm_type_of_lambda_is_lambda() {
    let src = "(type (fn (x) x))";
    assert_eq!(
        common::eval(src),
        Value::keyword("lambda"),
        "(type (fn ...)) should be :lambda"
    );
}

// ---------------------------------------------------------------------------
// 2026-05-29 audit — Pattern A: negative/oversized int -> usize guards.
// Each of these previously panicked (shift overflow / empty range), aborted
// (OOM allocation), or returned a silently-wrong result. They must now error
// cleanly on both backends.
// ---------------------------------------------------------------------------
eval_error_tests! {
    // STD-1
    bit_shift_left_overflow: "(bit/shift-left 1 64)" => "shift",
    bit_shift_left_negative: "(bit/shift-left 1 -1)" => "shift",
    bit_shift_right_overflow: "(bit/shift-right 1 64)" => "shift",
    bit_shift_right_negative: "(bit/shift-right 1 -1)" => "shift",
    // STD-2
    random_int_reversed_bounds: "(math/random-int 10 5)" => "math/random-int",
    // STD-4
    string_pad_left_negative: r#"(string/pad-left "x" -1)"# => "non-negative",
    string_pad_right_negative: r#"(string/pad-right "x" -1)"# => "non-negative",
    // STD-5
    list_chunk_negative: "(list/chunk -1 (list 1 2 3))" => "non-negative",
    list_split_at_negative: "(list/split-at (list 1 2 3) -1)" => "non-negative",
    list_sliding_negative: "(list/sliding (list 1 2 3) -1)" => "non-negative",
    list_times_negative: "(list/times -1 (lambda (i) i))" => "non-negative",
    list_repeat_negative: "(list/repeat -1 0)" => "non-negative",
    list_page_negative_per_page: "(list/page (list 1 2 3) 1 -1)" => "non-negative",
    list_pad_negative_len: "(list/pad (list 1) -1 0)" => "non-negative",
    // VM-3 (VM NTH opcode; tree-walker nth already guards)
    nth_negative_index: "(nth (list 1 2 3) -1)" => "non-negative",
}

// 2026-05-29 audit — Pattern B: UTF-8 byte slicing must not split a char.
// STD-3: text/chunk overlap on multibyte text previously panicked
// ("byte index N is not a char boundary"). It must return a list of strings.
eval_tests! {
    text_chunk_multibyte_overlap_no_panic:
        r#"(list? (text/chunk "λλλ λλλ λλλ λλλ λλλ λλλ" {:size 12 :overlap 3}))"# => Value::bool(true),
}

// 2026-05-29 audit — Wave 4 quasiquote/optimizer correctness.
eval_tests! {
    // EVAL-1: unquote-splicing must work inside vector templates.
    quasiquote_vector_splice:
        "(let ((xs (list 1 2))) `[0 ,@xs 3])"
        => Value::vector(vec![Value::int(0), Value::int(1), Value::int(2), Value::int(3)]),
    // EVAL-2: unquote must be honored inside map templates.
    quasiquote_map_unquote:
        r#"(let ((name "bob")) (:name `{:name ,name}))"# => Value::string("bob"),
}

eval_error_tests! {
    // EVAL-2: unquote-splicing has no meaning in a quasiquoted map, so it must
    // error clearly rather than leaking literal `(unquote-splicing ...)` data.
    quasiquote_map_value_splice_errors:
        "(let ((items (list 1 2 3))) `{:a 1 :items ,@items})"
        => "unquote-splicing is not allowed",
    quasiquote_map_key_splice_errors:
        "(let ((ks (list :a))) `{,@ks 1})"
        => "unquote-splicing is not allowed",
}

eval_error_tests! {
    // VM-4: a rest param named after a foldable builtin lexically shadows it,
    // so the optimizer must NOT constant-fold the body. Here the rest param `+`
    // is bound to the list (5), and calling it errors on both backends; the VM
    // previously folded `(+ 1 2)` to 3 and returned it.
    optimizer_rest_param_shadows_builtin: "((lambda (x . +) (+ 1 2)) 9 5)",
}

// Wide-integer runtime arithmetic: operands beyond the ±2^44 small-int fast-path
// range, applied via a lambda so the optimizer cannot constant-fold them. This
// exercises the vm_add/vm_sub/vm_mul fallback helpers at runtime (the small-int
// fast path and constant folding otherwise hide them). Regression for a coverage
// gap found by mutation testing (2026-06).
eval_tests! {
    wide_int_sub_runtime: "((fn (a b) (- a b)) 100000000000000 1)" => Value::int(99999999999999),
    wide_int_add_runtime: "((fn (a b) (+ a b)) 100000000000000 1)" => Value::int(100000000000001),
    wide_int_mul_runtime: "((fn (a b) (* a b)) 100000000000000 2)" => Value::int(200000000000000),
}

// ============================================================
// C1: `set!` through stdlib HOF callbacks must flow back to the
// captured local on BOTH backends. The VM previously closed open
// upvalues before every non-VM call and ran the callback on a fresh
// VM, so mutations were lost (returned 0 instead of 6). HOF callbacks
// are now routed back into the running VM.
// See docs/bugs/vm-set-lost-through-hof-callbacks.md and
// docs/plans/2026-06-18-c1-vm-hof-in-vm.md.
// ============================================================
eval_tests! {
    // The canonical repro from the bug report.
    hof_set_through_map: "(let ((c 0)) (map (fn (x) (set! c (+ c x))) (list 1 2 3)) c)" => Value::int(6),
    // filter callback mutating a captured accumulator.
    hof_set_through_filter:
        "(let ((c 0)) (filter (fn (x) (set! c (+ c x)) (even? x)) (list 1 2 3 4)) c)" => Value::int(10),
    // for-each callback mutating a captured accumulator.
    hof_set_through_for_each:
        "(let ((c 0)) (for-each (fn (x) (set! c (+ c x))) (list 10 20)) c)" => Value::int(30),
    // foldl callback with a `set!` side effect on a captured local.
    hof_set_through_foldl:
        "(let ((c 0)) (foldl (fn (acc x) (set! c (+ c x)) acc) 0 (list 1 2 3)) c)" => Value::int(6),
    // sort-by comparator that increments a captured counter once per key.
    hof_set_through_sort_by:
        "(let ((c 0)) (sort-by (fn (x) (set! c (+ c 1)) x) (list 3 1 2)) c)" => Value::int(3),
    // Nested HOFs: the inner map's callback mutates the outermost local.
    hof_set_through_nested_map:
        "(let ((c 0)) (map (fn (xs) (map (fn (y) (set! c (+ c y))) xs)) (list (list 1 2) (list 3 4))) c)"
        => Value::int(10),
    // Two distinct closures over the same captured local: one mutates via a
    // HOF, the other observes the mutation afterwards.
    hof_set_shared_local_observed_after:
        "(let ((c 0)) (define inc (fn () (set! c (+ c 1)))) (define get (fn () c)) (map (fn (x) (inc)) (list 1 2 3)) (get))"
        => Value::int(3),
    // The HOF still returns correct results while the callback mutates state.
    hof_map_result_unaffected_by_set:
        "(map (fn (x) (* x x)) (list 1 2 3))"
        => Value::list(vec![Value::int(1), Value::int(4), Value::int(9)]),
}

eval_error_tests! {
    // An error raised inside a HOF callback must propagate cleanly out of the
    // running VM (regression: the in-VM routing must unwind only the nested
    // frames, not corrupt the parent frame stack).
    hof_callback_error_propagates: r#"(map (fn (x) (error "boom")) (list 1 2 3))"#,
}

eval_tests! {
    // try/catch wrapping a HOF whose callback throws: both backends catch it.
    hof_callback_error_caught:
        r#"(try (map (fn (x) (error "boom")) (list 1)) (catch e :caught))"#
        => Value::keyword("caught"),

    // Regression: a throwing try/catch as a NON-FIRST binding in a parallel
    // `let` used to corrupt the operand stack — compile_let pushed earlier inits
    // without tracking stack_height, so the exception unwind truncated below
    // them and later local-slot access went out of bounds (crash on valid code,
    // found by the grammar fuzzer). See compiler.rs::compile_let.
    let_binding_throwing_try_nonfirst:
        r#"(let ((a 1) (b (try (throw 1) (catch e 2)))) b)"# => Value::int(2),
    let_binding_throwing_try_three:
        r#"(let ((a 0) (b (try (throw 1) (catch e 2))) (c 9)) (+ a b c))"# => Value::int(11),
    let_binding_throwing_try_uses_prior:
        r#"(let ((a 1) (b (try (throw a) (catch e 7)))) (+ a b))"# => Value::int(8),
    let_binding_nonthrowing_try_unaffected:
        r#"(let ((a 1) (b (try 5 (catch e 2)))) (+ a b))"# => Value::int(6),
}

// ============================================================
// Inlined string intrinsics (StringLength / StringRef / StringAppend opcodes)
// ============================================================

eval_tests! {
    // string-length: char count, not byte count.
    string_length_basic: r#"(string-length "hello")"# => Value::int(5),
    string_length_empty: r#"(string-length "")"# => Value::int(0),
    // Multi-byte chars count as one each (char-indexed, not byte-indexed).
    string_length_unicode: r#"(string-length "héllo")"# => Value::int(5),

    // string-ref: 0-based char indexing, returns a char.
    string_ref_first: r#"(string-ref "abc" 0)"# => Value::char('a'),
    string_ref_middle: r#"(string-ref "abc" 1)"# => Value::char('b'),
    string_ref_last: r#"(string-ref "abc" 2)"# => Value::char('c'),
    string_ref_unicode: r#"(string-ref "héllo" 1)"# => Value::char('é'),

    // string-append: 2-arg case (intrinsic); concatenation.
    string_append_basic: r#"(string-append "a" "bc")"# => Value::string("abc"),
    string_append_empty: r#"(string-append "" "x")"# => Value::string("x"),
    // Non-string arg is coerced via Display (matches stdlib semantics).
    string_append_coerce_num: r#"(string-append "n=" 42)"# => Value::string("n=42"),
    // N-ary string-append stays on the generic path and must still work.
    string_append_nary: r#"(string-append "a" "b" "c" "d")"# => Value::string("abcd"),
}

eval_error_tests! {
    // string-length on a non-string errors like the stdlib version.
    string_length_wrong_type: "(string-length 42)" => "expected string",
    // string-ref bounds / type / sign checks.
    string_ref_out_of_bounds: r#"(string-ref "abc" 5)"# => "out of bounds",
    string_ref_negative: r#"(string-ref "abc" -1)"# => "must be non-negative",
    string_ref_non_string: "(string-ref 42 0)" => "expected string",
    string_ref_non_int: r#"(string-ref "abc" "x")"# => "expected int",
}

// ============================================================
// stdlib additions: list/contains?, list/nth-or, list/take-last,
// list/drop-last, math/round-to, math/format-fixed, string/lines
// ============================================================

eval_tests! {
    list_contains_true: "(list/contains? (list 1 2 3) 2)" => Value::bool(true),
    list_contains_false: "(list/contains? (list 1 2 3) 9)" => Value::bool(false),
    list_nth_or_hit: "(list/nth-or (list 10 20 30) 1 -1)" => Value::int(20),
    list_nth_or_miss: "(list/nth-or (list 10 20 30) 9 -1)" => Value::int(-1),
    list_take_last_len: "(length (list/take-last 2 (list 1 2 3 4)))" => Value::int(2),
    list_take_last_first: "(first (list/take-last 2 (list 1 2 3 4)))" => Value::int(3),
    list_take_last_clamp: "(length (list/take-last 9 (list 1 2)))" => Value::int(2),
    list_drop_last_last: "(last (list/drop-last 2 (list 1 2 3 4)))" => Value::int(2),
    list_drop_last_clamp: "(length (list/drop-last 9 (list 1 2)))" => Value::int(0),
    math_round_to: "(math/round-to 3.14159 2)" => Value::float(3.14),
    math_round_to_zero: "(math/round-to 2.7 0)" => Value::float(3.0),
    math_format_fixed: r#"(math/format-fixed 1.2 3)"# => Value::string("1.200"),
    string_lines_count: r#"(length (string/lines "a\nb\r\nc\n"))"# => Value::int(3),
    string_lines_first: r#"(first (string/lines "x\ny"))"# => Value::string("x"),
}

// ============================================================
// Cycle collector (CORE-2, ADR #66): gc/collect + gc/stats and the
// plan §6-M3 adversarial shapes. Exact reclaim COUNTS are the trace
// model's business (unit-tested in sema-core::cycle and sized by
// leak_test.rs); these pin the semantic contract — garbage cycles
// are reclaimed, live closures are never severed by a collection.
// ============================================================

eval_tests! {
    gc_collect_returns_stats_map: "(map? (gc/collect))" => Value::bool(true),
    gc_stats_has_registry_size: "(integer? (:registry-size (gc/stats)))" => Value::bool(true),
    // Direct self-recursion (shape U): the churned closure's cell⇄closure
    // cycle is unreachable after the call and must be reclaimed.
    gc_self_recursive_local_collected: "(begin
        (define (churn)
          (define (loop n) (if (<= n 0) 0 (loop (- n 1))))
          (loop 3))
        (churn)
        (> (:collected (gc/collect)) 0))" => Value::bool(true),
    // Mutual local recursion: two cells, neither a self-capture — the shape
    // that defeats any weak-self-edge scheme (plan §4 option E).
    gc_mutual_local_recursion_collected: "(begin
        (define (churn)
          (define (ev? n) (if (<= n 0) true (od? (- n 1))))
          (define (od? n) (if (<= n 0) false (ev? (- n 1))))
          (ev? 4))
        (churn)
        (> (:collected (gc/collect)) 0))" => Value::bool(true),
    // set!-through-cell cycle: the back-edge is written after creation.
    gc_set_cell_cycle_collected: "(begin
        (define (churn)
          (define box nil)
          (define (grab) box)
          (set! box grab)
          nil)
        (churn)
        (> (:collected (gc/collect)) 0))" => Value::bool(true),
    // Live-closure guard: collecting mid-workload in a loop must never sever
    // a reachable closure's cells — the recursive local closure keeps working
    // across every pass (external strong counts protect it by construction).
    gc_live_closure_never_severed: "(begin
        (define (check n)
          (define (fact k) (if (<= k 1) 1 (* k (fact (- k 1)))))
          (if (<= n 0)
              (fact 10)
              (begin (gc/collect)
                     (assert (= (fact 5) 120) \"live closure severed\")
                     (check (- n 1)))))
        (check 20))" => Value::int(3628800),
    // A live mutual pair also survives collection (cells stay intact).
    gc_live_mutual_pair_survives: "(begin
        (define (make)
          (define (ev? n) (if (<= n 0) true (od? (- n 1))))
          (define (od? n) (if (<= n 0) false (ev? (- n 1))))
          ev?)
        (define keep (make))
        (gc/collect)
        (keep 7))" => Value::bool(false),
}

// ============================================================
// Prelude macro-name collisions
// (docs/bugs/prelude-macro-names-collide-with-user-defines.md)
// ============================================================
// Macro expansion rewrites ANY list whose head names a macro — define-sugar
// heads included — and local defines cannot shadow a macro at call sites.
// These pin (a) that the nested define-with-`let` shapes compile fine with a
// non-colliding name (no lowering/resolution bug hides here), and (b) the
// collision itself, so the write-up stays honest until the expander gains
// binding-position awareness or scope-aware shadowing.

eval_tests! {
    nested_define_with_let_body_in_lambda: "(begin
        (define (outer a) (fn () (define (stp n) (let ((v 1)) v)) (stp 3)))
        ((outer 1)))" => Value::int(1),
    nested_define_with_let_body_direct: "(begin
        (define (outer a) (define (stp n) (let ((v 1)) v)) (stp 3))
        (outer 1))" => Value::int(1),
}

eval_error_tests! {
    // `step` is the prelude workflow macro; its expansion (a `let` template)
    // lands in the define head, so define sugar sees no symbol.
    prelude_macro_name_in_define_sugar_head_errors:
        "(define (step n) n)" => "define: expected a symbol",
}
