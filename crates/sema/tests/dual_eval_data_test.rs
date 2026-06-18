mod common;

use sema_core::Value;

// ============================================================
// JSON encode/decode — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    json_encode_map: r#"(json/encode {:a 1})"# => Value::string(r#"{"a":1}"#),
    json_encode_vector: r#"(json/encode [1 2 3])"# => Value::string("[1,2,3]"),
    json_encode_nested: r#"(json/encode {:a [1 2]})"# => Value::string(r#"{"a":[1,2]}"#),
    json_encode_nil: "(json/encode nil)" => Value::string("null"),
    json_encode_bool_true: "(json/encode #t)" => Value::string("true"),
    json_encode_bool_false: "(json/encode #f)" => Value::string("false"),
    json_encode_string: r#"(json/encode "hello")"# => Value::string(r#""hello""#),
    json_encode_int: "(json/encode 42)" => Value::string("42"),
    json_encode_float: "(json/encode 3.14)" => Value::string("3.14"),
    json_encode_list: r#"(json/encode '(1 2 3))"# => Value::string("[1,2,3]"),
    json_decode_object: r#"(get (json/decode "{\"a\":1}") :a)"# => Value::int(1),
    json_decode_array: r#"(length (json/decode "[1,2,3]"))"# => Value::int(3),
    json_decode_null: r#"(json/decode "null")"# => Value::nil(),
    json_decode_bool: r#"(json/decode "true")"# => Value::bool(true),
    json_decode_string: r#"(json/decode "\"hello\"")"# => Value::string("hello"),
    json_decode_number: r#"(json/decode "42")"# => Value::int(42),
    json_roundtrip_map: r#"(get (json/decode (json/encode {:a 1})) :a)"# => Value::int(1),
    json_roundtrip_vector: r#"(length (json/decode (json/encode [1 2 3])))"# => Value::int(3),
}

// ============================================================
// Regex operations — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    regex_match_found: r#"(get (regex/match "\\d+" "abc123def") :match)"# => Value::string("123"),
    regex_match_no_match: r#"(regex/match "\\d+" "abc")"# => Value::nil(),
    regex_find_all: r#"(length (regex/find-all "\\d+" "a1b2c3"))"# => Value::int(3),
    regex_find_all_empty: r#"(regex/find-all "\\d+" "abc")"# => Value::list(vec![]),
    regex_replace: r#"(regex/replace "\\d+" "X" "a1b2c3")"# => Value::string("aXb2c3"),
    regex_replace_all: r#"(regex/replace-all "\\s+" " " "a  b   c")"# => Value::string("a b c"),
    regex_split: r#"(length (regex/split "," "a,b,c"))"# => Value::int(3),
    regex_split_whitespace: r#"(length (regex/split "\\s+" "a b c"))"# => Value::int(3),
}

// ============================================================
// CSV operations — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    csv_parse: r#"(length (csv/parse "a,b\n1,2\n3,4"))"# => Value::int(3),
    csv_parse_maps: r#"(get (car (csv/parse-maps "a,b\n1,2")) :a)"# => Value::string("1"),
}

// ============================================================
// Format — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    format_basic: r#"(format "~a" 42)"# => Value::string("42"),
    format_multiple: r#"(format "~a+~a=~a" 1 2 3)"# => Value::string("1+2=3"),
}

// ============================================================
// Hash functions — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    hash_sha256: r#"(string? (hash/sha256 "test"))"# => Value::bool(true),
    hash_md5: r#"(string? (hash/md5 "test"))"# => Value::bool(true),
    hash_md5_length: r#"(string-length (hash/md5 "test"))"# => Value::int(32),
    hash_sha256_deterministic: r#"(equal? (hash/sha256 "test") (hash/sha256 "test"))"# => Value::bool(true),
}

// ============================================================
// Type operations — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    type_int: "(type 42)" => Value::keyword("int"),
    type_float: "(type 3.14)" => Value::keyword("float"),
    type_string: r#"(type "hi")"# => Value::keyword("string"),
    type_bool: "(type #t)" => Value::keyword("bool"),
    type_nil: "(type nil)" => Value::keyword("nil"),
    type_list: "(type '(1 2))" => Value::keyword("list"),
    type_vector: "(type [1 2])" => Value::keyword("vector"),
    type_map: "(type {:a 1})" => Value::keyword("map"),
    type_keyword: "(type :foo)" => Value::keyword("keyword"),
    type_symbol: "(type 'foo)" => Value::keyword("symbol"),
    type_char: r#"(type #\a)"# => Value::keyword("char"),
    type_fn: "(type +)" => Value::keyword("native-fn"),
}

// ============================================================
// Predicates — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    pred_number: "(number? 42)" => Value::bool(true),
    pred_number_str: r#"(number? "hi")"# => Value::bool(false),
    pred_string: r#"(string? "hi")"# => Value::bool(true),
    pred_list: "(list? '(1 2))" => Value::bool(true),
    pred_vector: "(vector? [1 2])" => Value::bool(true),
    pred_map: "(map? {:a 1})" => Value::bool(true),
    pred_nil: "(nil? nil)" => Value::bool(true),
    pred_bool: "(boolean? #t)" => Value::bool(true),
    pred_symbol: "(symbol? 'foo)" => Value::bool(true),
    pred_keyword: "(keyword? :foo)" => Value::bool(true),
    pred_fn: "(fn? +)" => Value::bool(true),
    pred_pair: "(pair? '(1 2))" => Value::bool(true),
    pred_empty_list: "(empty? '())" => Value::bool(true),
    pred_not_empty: "(empty? '(1))" => Value::bool(false),
    pred_zero: "(zero? 0)" => Value::bool(true),
    pred_positive: "(positive? 5)" => Value::bool(true),
    pred_negative: "(negative? -3)" => Value::bool(true),
    pred_even: "(even? 4)" => Value::bool(true),
    pred_odd: "(odd? 3)" => Value::bool(true),
    pred_integer: "(integer? 42)" => Value::bool(true),
    pred_float: "(float? 3.14)" => Value::bool(true),
}
