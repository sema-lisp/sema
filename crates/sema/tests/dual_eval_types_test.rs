#![allow(clippy::approx_constant)]
mod common;

use sema_core::Value;

// ============================================================
// Char operations — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    char_literal: r#"#\a"# => Value::char('a'),
    char_pred: r#"(char? #\a)"# => Value::bool(true),
    char_alpha: r#"(char-alphabetic? #\a)"# => Value::bool(true),
    char_numeric: r#"(char-numeric? #\5)"# => Value::bool(true),
    char_whitespace: r#"(char-whitespace? #\space)"# => Value::bool(true),
    char_upper: r#"(char-upcase #\a)"# => Value::char('A'),
    char_to_int: r#"(char->integer #\a)"# => Value::int(97),
    int_to_char: r#"(integer->char 65)"# => Value::char('A'),
    char_cmp_lt: r#"(char<? #\a #\b)"# => Value::bool(true),
    char_cmp_eq: r#"(char=? #\a #\a)"# => Value::bool(true),
}

// ============================================================
// Bytevector operations — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    bv_make: "(bytevector-length (make-bytevector 5 0))" => Value::int(5),
    bv_pred: "(bytevector? (make-bytevector 3 0))" => Value::bool(true),
    bv_ref: "(bytevector-u8-ref (bytevector 10 20 30) 1)" => Value::int(20),
    bv_length: "(bytevector-length (bytevector 1 2 3))" => Value::int(3),
    bv_copy: "(bytevector-length (bytevector-copy (bytevector 1 2 3)))" => Value::int(3),
    bv_append: "(bytevector-length (bytevector-append (bytevector 1 2) (bytevector 3 4)))" => Value::int(4),
    bv_list_roundtrip: "(bytevector->list (list->bytevector '(10 20 30)))" => Value::list(vec![Value::int(10), Value::int(20), Value::int(30)]),
    bv_utf8: r#"(utf8->string (string->utf8 "hello"))"# => Value::string("hello"),
    bv_display: "(bytevector? (bytevector 1 2 3))" => Value::bool(true),
}

// ============================================================
// Base64 — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    base64_encode: r#"(base64/encode "hello")"# => Value::string("aGVsbG8="),
    base64_decode: r#"(base64/decode "aGVsbG8=")"# => Value::string("hello"),
    base64_empty: r#"(base64/encode "")"# => Value::string(""),
    base64_roundtrip: r#"(base64/decode (base64/encode "Sema Lisp"))"# => Value::string("Sema Lisp"),
    base64_encode_bytes: "(base64/encode-bytes (bytevector 72 101 108 108 111))" => Value::string("SGVsbG8="),
    base64_decode_bytes: r#"(bytevector-length (base64/decode-bytes "SGVsbG8="))"# => Value::int(5),
}

// ============================================================
// Bitwise operations — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    bit_and: "(bit/and 12 10)" => Value::int(8),
    bit_or: "(bit/or 12 10)" => Value::int(14),
    bit_xor: "(bit/xor 12 10)" => Value::int(6),
    bit_not: "(bit/not 0)" => Value::int(-1),
    bit_shl: "(bit/shift-left 1 4)" => Value::int(16),
    bit_shr: "(bit/shift-right 16 4)" => Value::int(1),
    bit_xor_self: "(bit/xor 42 42)" => Value::int(0),
}

// ============================================================
// Delay/Force (promises) — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    delay_basic: "(force (delay 42))" => Value::int(42),
    delay_is_promise: "(promise? (delay 42))" => Value::bool(true),
    delay_memoize: "(begin (define p (delay (+ 1 2))) (force p) (force p))" => Value::int(3),
    // (force 42) — non-promise — now errors (Wave 6a/D4); see dual_eval_test::force_non_promise_errors
    promise_forced: "(begin (define p (delay 99)) (force p) (promise-forced? p))" => Value::bool(true),
}

// ============================================================
// Define-record-type — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    record_basic: "(begin (define-record-type point (make-point x y) point? (x point-x) (y point-y)) (point-x (make-point 3 4)))" => Value::int(3),
    record_pred: "(begin (define-record-type point (make-point x y) point? (x point-x) (y point-y)) (point? (make-point 1 2)))" => Value::bool(true),
    record_pred_false: "(begin (define-record-type point (make-point x y) point? (x point-x) (y point-y)) (point? 42))" => Value::bool(false),
    record_equality: "(begin (define-record-type point (make-point x y) point? (x point-x) (y point-y)) (equal? (make-point 1 2) (make-point 1 2)))" => Value::bool(true),
}

// ============================================================
// Embedding operations — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    emb_length: "(embedding/length (embedding/list->embedding '(1.0 2.0 3.0)))" => Value::int(3),
    emb_ref: "(embedding/ref (embedding/list->embedding '(10.5 20.5 30.5)) 1)" => Value::float(20.5),
    emb_roundtrip: "(length (embedding/->list (embedding/list->embedding '(1.0 2.0 3.0))))" => Value::int(3),
    emb_int_coerce: "(embedding/ref (embedding/list->embedding '(42)) 0)" => Value::float(42.0),
    // Approximate check: cosine similarity of identical vectors should be ~1.0 but exact f64 == 1.0 is fragile
    emb_similarity: "(let ((e (embedding/list->embedding '(1.0 0.0 0.0)))) (> (llm/similarity e e) 0.99))" => Value::bool(true),
    emb_orthogonal: "(llm/similarity (embedding/list->embedding '(1.0 0.0)) (embedding/list->embedding '(0.0 1.0)))" => Value::float(0.0),
}

// ============================================================
// Type conversions — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    int_to_float: "(float 42)" => Value::float(42.0),
    float_to_int: "(int 3.7)" => Value::int(3),
    string_to_num: r#"(string->number "42")"# => Value::int(42),
    string_to_float: r#"(string->number "3.14")"# => Value::float(3.14),
    num_to_string: "(number->string 42)" => Value::string("42"),
    sym_to_string: r#"(symbol->string 'foo)"# => Value::string("foo"),
    string_to_sym: r#"(symbol? (string->symbol "foo"))"# => Value::bool(true),
    kw_to_string: r#"(keyword->string :foo)"# => Value::string("foo"),
    string_to_kw: r#"(keyword? (string->keyword "foo"))"# => Value::bool(true),
}

// ============================================================
// UUID — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    uuid_is_string: "(string? (uuid/v4))" => Value::bool(true),
    uuid_length: "(string-length (uuid/v4))" => Value::int(36),
}

// ============================================================
// Frequencies — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    freq_basic: "(get (frequencies '(:a :b :a)) :a)" => Value::int(2),
    freq_empty: "(count (frequencies '()))" => Value::int(0),
}

// ============================================================
// Error tests — dual eval (tree-walker + VM)
// ============================================================

dual_eval_error_tests! {
    err_emb_oob: "(embedding/ref (embedding/list->embedding '(1.0 2.0)) 5)",
}

// ============================================================
// Float edge cases — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    // -0.0 vs +0.0 equality (IEEE 754: they are equal)
    neg_zero_equals_pos_zero: "(= -0.0 0.0)" => Value::bool(true),
    neg_zero_equal_fn: "(equal? -0.0 0.0)" => Value::bool(true),
    neg_zero_arithmetic: "(= (* -1.0 0.0) 0.0)" => Value::bool(true),
    neg_zero_negate: "(= (- 0.0) 0.0)" => Value::bool(true),
    neg_zero_is_zero: "(zero? -0.0)" => Value::bool(true),
    neg_zero_eq_identity: "(eq? -0.0 0.0)" => Value::bool(true),

    // -0.0 in collections — lookup must work since equal? says equal
    neg_zero_in_list: "(member -0.0 '(1.0 0.0 2.0))" => Value::list(vec![Value::float(0.0), Value::float(2.0)]),
}
