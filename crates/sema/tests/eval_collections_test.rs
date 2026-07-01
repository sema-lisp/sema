mod common;

use sema_core::Value;

// ============================================================
// String operations
// ============================================================

eval_tests! {
    str_upper: r#"(string/upper "hello")"# => Value::string("HELLO"),
    str_lower: r#"(string/lower "HELLO")"# => Value::string("hello"),
    str_trim: r#"(string/trim "  hi  ")"# => Value::string("hi"),
    str_trim_left: r#"(string/trim-left "  hi  ")"# => Value::string("hi  "),
    str_trim_right: r#"(string/trim-right "  hi  ")"# => Value::string("  hi"),
    str_split: r#"(length (string/split "a,b,c" ","))"# => Value::int(3),
    str_join: r#"(string/join '("a" "b" "c") "-")"# => Value::string("a-b-c"),
    str_contains: r#"(string/contains? "hello world" "world")"# => Value::bool(true),
    str_contains_false: r#"(string/contains? "hello" "xyz")"# => Value::bool(false),
    str_starts_with: r#"(string/starts-with? "hello" "hel")"# => Value::bool(true),
    str_ends_with: r#"(string/ends-with? "hello" "llo")"# => Value::bool(true),
    str_replace: r#"(string/replace "hello world" "world" "rust")"# => Value::string("hello rust"),
    str_repeat: r#"(string/repeat "ab" 3)"# => Value::string("ababab"),
    str_reverse: r#"(string/reverse "hello")"# => Value::string("olleh"),
    str_length: r#"(string-length "hello")"# => Value::int(5),
    str_append: r#"(string-append "hello" " " "world")"# => Value::string("hello world"),
    str_substring: r#"(substring "hello" 1 3)"# => Value::string("el"),
    str_index_of: r#"(string/index-of "hello" "ll")"# => Value::int(2),
    str_index_of_unicode: r#"(string/index-of "café world" "world")"# => Value::int(5),
    str_index_of_emoji: r#"(string/index-of "hello 🌍 world" "world")"# => Value::int(8),
    str_index_of_not_found: r#"(string/index-of "hello" "xyz")"# => Value::nil(),
    str_last_index_of: r#"(string/last-index-of "abcabc" "bc")"# => Value::int(4),
    str_last_index_of_unicode: r#"(string/last-index-of "café café" "café")"# => Value::int(5),
    str_last_index_of_not_found: r#"(string/last-index-of "hello" "xyz")"# => Value::nil(),
    str_capitalize: r#"(string/capitalize "hello world")"# => Value::string("Hello world"),
    str_empty: r#"(string/empty? "")"# => Value::bool(true),
    str_not_empty: r#"(string/empty? "hi")"# => Value::bool(false),
    str_pad_left: r#"(string/pad-left "42" 5 "0")"# => Value::string("00042"),
    str_pad_right: r#"(string/pad-right "42" 5 "0")"# => Value::string("42000"),
    str_chars: r#"(length (string/chars "hello"))"# => Value::int(5),
}

// ============================================================
// String conversion functions
// ============================================================

eval_tests! {
    str_to_number: r#"(string->number "42")"# => Value::int(42),
    str_to_symbol: r#"(symbol? (string->symbol "foo"))"# => Value::bool(true),
    number_to_str: "(number->string 42)" => Value::string("42"),
    symbol_to_str: r#"(symbol->string 'foo)"# => Value::string("foo"),
    keyword_to_str: r#"(keyword->string :foo)"# => Value::string("foo"),
    str_to_keyword: r#"(keyword? (string->keyword "foo"))"# => Value::bool(true),
}

// ============================================================
// List operations
// ============================================================

eval_tests! {
    // Foundational ops: hand-constructed expected values so the oracle does not
    // depend on the tree-walker (see docs/bugs/eval-tw-oracle-circularity.md).
    list_car: "(car '(1 2 3))" => Value::int(1),
    list_cdr: "(cdr '(1 2 3))" => Value::list(vec![Value::int(2), Value::int(3)]),
    list_cons: "(cons 1 '(2 3))" => Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
    list_length: "(length '(1 2 3))" => Value::int(3),
    list_append: "(append '(1 2) '(3 4))" => Value::list(vec![Value::int(1), Value::int(2), Value::int(3), Value::int(4)]),
    list_reverse: "(reverse '(1 2 3))" => Value::list(vec![Value::int(3), Value::int(2), Value::int(1)]),
    list_map: "(map (fn (x) (* x 2)) '(1 2 3))" => common::eval("'(2 4 6)"),
    list_filter: "(filter odd? '(1 2 3 4 5))" => common::eval("'(1 3 5)"),
    list_foldl: "(foldl + 0 '(1 2 3))" => Value::int(6),
    // Order-sensitive folds: a non-commutative function pins foldl's accumulator
    // arg order ((acc, item), left-to-right). All prior foldl tests used `+`, so
    // swapping the callback's args was invisible (coverage gap found by mutation
    // testing 2026-06).
    list_foldl_subtract: "(foldl (fn (acc x) (- acc x)) 0 '(1 2 3))" => Value::int(-6),
    list_foldl_builds_reversed: "(foldl (fn (acc x) (cons x acc)) '() '(1 2 3))" => common::eval("'(3 2 1)"),
    list_foldr: "(foldr cons '() '(1 2 3))" => common::eval("'(1 2 3)"),
    list_sort: "(sort '(3 1 2))" => common::eval("'(1 2 3)"),
    // Mixed int/float must order by numeric value, not by internal tag (which
    // would group all ints before all floats and misplace 1.5).
    list_sort_mixed_numbers: "(sort (list 3 1.5 2))" => common::eval("'(1.5 2 3)"),
    list_sort_strings: r#"(sort (list "banana" "apple" "cherry"))"# => common::eval(r#"'("apple" "banana" "cherry")"#),
    list_sort_by: "(sort-by (fn (x) (- 0 x)) '(3 1 2))" => common::eval("'(3 2 1)"),
    list_flatten: "(flatten '(1 (2 3) (4 5)))" => common::eval("'(1 2 3 4 5)"),
    list_flatten_deep: "(flatten-deep '(1 (2 3) (4 (5))))" => common::eval("'(1 2 3 4 5)"),
    list_zip: "(zip '(1 2 3) '(4 5 6))" => common::eval("'((1 4) (2 5) (3 6))"),
    list_take: "(take 2 '(1 2 3 4))" => common::eval("'(1 2)"),
    list_drop: "(drop 2 '(1 2 3 4))" => common::eval("'(3 4)"),
    list_nth: "(nth '(10 20 30) 1)" => Value::int(20),
    list_last: "(last '(1 2 3))" => Value::int(3),
    list_range: "(range 1 5)" => common::eval("'(1 2 3 4)"),
    list_range_step: "(range 0 10 3)" => common::eval("'(0 3 6 9)"),
    list_unique: "(sort (list/unique '(1 2 2 3 3 3)))" => common::eval("'(1 2 3)"),
    list_find: "(list/find odd? '(2 4 5 6))" => Value::int(5),
    list_count: "(count '(1 2 3))" => Value::int(3),
    list_empty: "(empty? '())" => Value::bool(true),
    list_partition: "(length (car (partition even? '(1 2 3 4 5))))" => Value::int(2),
    list_flat_map: "(flat-map (fn (x) (list x x)) '(1 2 3))" => common::eval("'(1 1 2 2 3 3)"),
    list_for_each: "(begin (define acc '()) (for-each (fn (x) (set! acc (cons x acc))) '(1 2 3)) (reverse acc))" => common::eval("'(1 2 3)"),
    list_member: "(member 2 '(1 2 3))" => common::eval("'(2 3)"),
    list_member_missing: "(member 5 '(1 2 3))" => Value::bool(false),
    list_reduce: "(reduce + '(1 2 3 4))" => Value::int(10),
    list_iota: "(iota 5)" => common::eval("'(0 1 2 3 4)"),
    list_interpose: r#"(interpose ", " '("a" "b" "c"))"# => common::eval(r#"'("a" ", " "b" ", " "c")"#),
}

// ============================================================
// Vector operations
// ============================================================

eval_tests! {
    vec_nth: "(nth [10 20 30] 1)" => Value::int(20),
    vec_length: "(length [1 2 3])" => Value::int(3),
    vec_to_list: "(vector->list [1 2 3])" => Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
    list_to_vec: "(vector? (list->vector '(1 2 3)))" => Value::bool(true),
}

// ============================================================
// Apply
// ============================================================

eval_tests! {
    apply_basic: "(apply + '(1 2 3))" => Value::int(6),
    apply_prefix: "(apply + 1 2 '(3 4))" => Value::int(10),
}

// ============================================================
// any / every
// ============================================================

eval_tests! {
    any_found: "(any odd? '(2 4 5 6))" => Value::bool(true),
    any_none: "(any odd? '(2 4 6))" => Value::bool(false),
    any_empty: "(any odd? '())" => Value::bool(false),
    every_true: "(every even? '(2 4 6))" => Value::bool(true),
    every_false: "(every even? '(2 3 6))" => Value::bool(false),
    every_empty: "(every even? '())" => Value::bool(true),
}

// ============================================================
// list/index-of
// ============================================================

eval_tests! {
    index_of_found: "(list/index-of '(10 20 30) 20)" => Value::int(1),
    index_of_first: "(list/index-of '(10 20 30) 10)" => Value::int(0),
    index_of_last: "(list/index-of '(10 20 30) 30)" => Value::int(2),
    index_of_missing: "(list/index-of '(10 20 30) 99)" => Value::nil(),
    index_of_empty: "(list/index-of '() 1)" => Value::nil(),
    index_of_duplicate: "(list/index-of '(1 2 2 3) 2)" => Value::int(1),
}

// ============================================================
// list/group-by
// ============================================================

eval_tests! {
    group_by_even_odd: "(length (keys (list/group-by even? '(1 2 3 4 5))))" => Value::int(2),
    group_by_empty: "(length (keys (list/group-by even? '())))" => Value::int(0),
    group_by_values: "(length (hashmap/get (list/group-by even? '(1 2 3 4 5)) #f))" => Value::int(3),
}

// ============================================================
// list/interleave
// ============================================================

eval_tests! {
    interleave_basic: "(list/interleave '(1 3 5) '(2 4 6))" => common::eval("'(1 2 3 4 5 6)"),
    interleave_truncate: "(list/interleave '(1 3 5) '(2 4))" => common::eval("'(1 2 3 4)"),
    interleave_three: "(list/interleave '(1 4) '(2 5) '(3 6))" => common::eval("'(1 2 3 4 5 6)"),
    interleave_empty_first: "(list/interleave '() '(1 2 3))" => common::eval("'()"),
    interleave_empty_second: "(list/interleave '(1 2 3) '())" => common::eval("'()"),
}

// ============================================================
// list/chunk
// ============================================================

eval_tests! {
    chunk_even: "(list/chunk 2 '(1 2 3 4))" => common::eval("'((1 2) (3 4))"),
    chunk_uneven: "(list/chunk 2 '(1 2 3 4 5))" => common::eval("'((1 2) (3 4) (5))"),
    chunk_larger: "(list/chunk 10 '(1 2 3))" => common::eval("'((1 2 3))"),
    chunk_one: "(list/chunk 1 '(1 2 3))" => common::eval("'((1) (2) (3))"),
    chunk_empty: "(list/chunk 2 '())" => common::eval("'()"),
}

eval_error_tests! {
    chunk_zero: "(list/chunk 0 '(1 2 3))",
    // Heterogeneous `sort` (no comparator) is a type error rather than a silent,
    // tag-ordered nonsense result. Numbers are one family; other types aren't.
    sort_mixed_int_string: r#"(sort (list 3 "a" 1))"#,
    sort_mixed_number_map: "(sort (list 1 {:k 1}))",
}

// ============================================================
// take-while / drop-while
// ============================================================

eval_tests! {
    take_while_basic: "(take-while even? '(2 4 5 6))" => common::eval("'(2 4)"),
    take_while_none: "(take-while even? '(1 2 4))" => common::eval("'()"),
    take_while_all: "(take-while even? '(2 4 6))" => common::eval("'(2 4 6)"),
    take_while_empty: "(take-while even? '())" => common::eval("'()"),
    drop_while_basic: "(drop-while even? '(2 4 5 6))" => common::eval("'(5 6)"),
    drop_while_none: "(drop-while even? '(1 2 4))" => common::eval("'(1 2 4)"),
    drop_while_all: "(drop-while even? '(2 4 6))" => common::eval("'()"),
    drop_while_empty: "(drop-while even? '())" => common::eval("'()"),
}

// ============================================================
// list/take-while / list/drop-while
// ============================================================

eval_tests! {
    list_take_while_basic: "(list/take-while even? '(2 4 5 6))" => common::eval("'(2 4)"),
    list_take_while_empty: "(list/take-while even? '())" => common::eval("'()"),
    list_drop_while_basic: "(list/drop-while even? '(2 4 5 6))" => common::eval("'(5 6)"),
    list_drop_while_empty: "(list/drop-while even? '())" => common::eval("'()"),
}

// ============================================================
// list/dedupe — CONSECUTIVE duplicates only
// ============================================================

eval_tests! {
    dedupe_consecutive: "(list/dedupe '(1 1 2 1 1))" => common::eval("'(1 2 1)"),
    dedupe_no_dupes: "(list/dedupe '(1 2 3))" => common::eval("'(1 2 3)"),
    dedupe_all_same: "(list/dedupe '(5 5 5))" => common::eval("'(5)"),
    dedupe_empty: "(list/dedupe '())" => common::eval("'()"),
    dedupe_single: "(list/dedupe '(42))" => common::eval("'(42)"),
    dedupe_strings: r#"(list/dedupe '("a" "a" "b" "b" "a"))"# => common::eval(r#"'("a" "b" "a")"#),
}

// ============================================================
// list/split-at
// ============================================================

eval_tests! {
    split_at_middle: "(list/split-at '(1 2 3 4 5) 3)" => common::eval("'((1 2 3) (4 5))"),
    split_at_zero: "(list/split-at '(1 2 3) 0)" => common::eval("'(() (1 2 3))"),
    split_at_end: "(list/split-at '(1 2 3) 3)" => common::eval("'((1 2 3) ())"),
    split_at_beyond: "(list/split-at '(1 2 3) 10)" => common::eval("'((1 2 3) ())"),
    split_at_empty: "(list/split-at '() 0)" => common::eval("'(() ())"),
}

// ============================================================
// list/sum
// ============================================================

eval_tests! {
    sum_ints: "(list/sum '(1 2 3))" => Value::int(6),
    sum_mixed: "(list/sum '(1 2.0 3))" => Value::float(6.0),
    sum_floats: "(list/sum '(1.5 2.5))" => Value::float(4.0),
    sum_empty: "(list/sum '())" => Value::int(0),
    sum_single: "(list/sum '(42))" => Value::int(42),
    sum_negative: "(list/sum '(-1 -2 3))" => Value::int(0),
}

// ============================================================
// list/min / list/max
// ============================================================

eval_tests! {
    min_basic: "(list/min '(3 1 2))" => Value::int(1),
    min_single: "(list/min '(5))" => Value::int(5),
    min_negative: "(list/min '(-3 -1 -2))" => Value::int(-3),
    min_mixed: "(list/min '(3 1.5 2))" => Value::float(1.5),
    max_basic: "(list/max '(3 1 2))" => Value::int(3),
    max_single: "(list/max '(5))" => Value::int(5),
    max_negative: "(list/max '(-3 -1 -2))" => Value::int(-1),
    max_mixed: "(list/max '(3 1.5 2))" => Value::int(3),
}

eval_error_tests! {
    min_empty: "(list/min '())",
    max_empty: "(list/max '())",
}

// ============================================================
// list/repeat / make-list
// ============================================================

eval_tests! {
    repeat_basic: "(list/repeat 3 0)" => common::eval("'(0 0 0)"),
    repeat_string: r#"(list/repeat 2 "hi")"# => common::eval(r#"'("hi" "hi")"#),
    repeat_zero: "(list/repeat 0 1)" => common::eval("'()"),
    make_list_basic: "(make-list 3 #t)" => common::eval("'(#t #t #t)"),
}

// ============================================================
// list/reject
// ============================================================

eval_tests! {
    reject_basic: "(list/reject even? '(1 2 3 4 5))" => common::eval("'(1 3 5)"),
    reject_none: "(list/reject even? '(1 3 5))" => common::eval("'(1 3 5)"),
    reject_all: "(list/reject even? '(2 4 6))" => common::eval("'()"),
    reject_empty: "(list/reject even? '())" => common::eval("'()"),
}

// ============================================================
// list/pluck
// ============================================================

eval_tests! {
    pluck_basic: r#"(list/pluck :name (list {:name "a"} {:name "b"}))"# => common::eval(r#"'("a" "b")"#),
    pluck_missing_key: r#"(list/pluck :age (list {:name "a"}))"# => common::eval("'(nil)"),
    pluck_empty: "(list/pluck :x '())" => common::eval("'()"),
}

// ============================================================
// list/avg
// ============================================================

eval_tests! {
    avg_ints: "(list/avg '(2 4 6))" => Value::float(4.0),
    avg_mixed: "(list/avg '(1 2.0 3))" => Value::float(2.0),
    avg_single: "(list/avg '(10))" => Value::float(10.0),
}

eval_error_tests! {
    avg_empty: "(list/avg '())",
}

// ============================================================
// list/median
// ============================================================

eval_tests! {
    median_odd: "(list/median '(3 1 2))" => Value::float(2.0),
    median_even: "(list/median '(3 1 2 4))" => Value::float(2.5),
    median_single: "(list/median '(7))" => Value::float(7.0),
    median_sorted: "(list/median '(1 2 3 4 5))" => Value::float(3.0),
}

eval_error_tests! {
    median_empty: "(list/median '())",
}

// ============================================================
// list/mode
// ============================================================

eval_tests! {
    mode_single_mode: "(list/mode '(1 2 2 3))" => Value::int(2),
    // NOTE: Depends on BTreeMap iteration order for tie-breaking — tied modes are returned
    // sorted by key because the implementation iterates a BTreeMap (or sorted frequency map).
    // Wrapping in (sort ...) to make order-independence explicit.
    mode_tie_sorted: "(sort (list/mode '(2 1 2 1 3)))" => Value::list(vec![Value::int(1), Value::int(2)]),
    mode_all_same: "(list/mode '(5 5 5))" => Value::int(5),
    // NOTE: Same BTreeMap order dependency as mode_tie_sorted — all elements have equal frequency.
    mode_all_unique: "(sort (list/mode '(1 2 3)))" => Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
    mode_single_element: "(list/mode '(42))" => Value::int(42),
}

eval_error_tests! {
    mode_empty: "(list/mode '())",
}

// ============================================================
// list/diff
// ============================================================

eval_tests! {
    diff_basic: "(list/diff '(1 2 3 4) '(2 4))" => common::eval("'(1 3)"),
    diff_removes_all: "(list/diff '(1 2 2 3) '(2))" => common::eval("'(1 3)"),
    diff_no_overlap: "(list/diff '(1 2 3) '(4 5))" => common::eval("'(1 2 3)"),
    diff_all_overlap: "(list/diff '(1 2 3) '(1 2 3))" => common::eval("'()"),
    diff_empty_first: "(list/diff '() '(1 2))" => common::eval("'()"),
    diff_empty_second: "(list/diff '(1 2 3) '())" => common::eval("'(1 2 3)"),
}

// ============================================================
// list/intersect
// ============================================================

eval_tests! {
    intersect_basic: "(list/intersect '(1 2 3) '(2 3 4))" => common::eval("'(2 3)"),
    intersect_preserves_dupes: "(list/intersect '(1 2 2 3) '(2 4))" => common::eval("'(2 2)"),
    intersect_no_overlap: "(list/intersect '(1 2) '(3 4))" => common::eval("'()"),
    intersect_empty_first: "(list/intersect '() '(1 2))" => common::eval("'()"),
    intersect_empty_second: "(list/intersect '(1 2) '())" => common::eval("'()"),
}

// ============================================================
// list/sliding
// ============================================================

eval_tests! {
    sliding_basic: "(list/sliding '(1 2 3 4 5) 3)" => common::eval("'((1 2 3) (2 3 4) (3 4 5))"),
    sliding_full: "(list/sliding '(1 2 3) 3)" => common::eval("'((1 2 3))"),
    sliding_one: "(list/sliding '(1 2 3) 1)" => common::eval("'((1) (2) (3))"),
    sliding_too_big: "(list/sliding '(1 2) 5)" => common::eval("'()"),
    sliding_step: "(list/sliding '(1 2 3 4 5) 2 2)" => common::eval("'((1 2) (3 4))"),
    sliding_step3: "(list/sliding '(1 2 3 4 5 6) 2 3)" => common::eval("'((1 2) (4 5))"),
    sliding_empty: "(list/sliding '() 2)" => common::eval("'()"),
}

eval_error_tests! {
    sliding_zero_size: "(list/sliding '(1 2 3) 0)",
    sliding_zero_step: "(list/sliding '(1 2 3) 2 0)",
}

// ============================================================
// list/key-by
// ============================================================

eval_tests! {
    key_by_basic: r#"(hashmap/get (list/key-by (fn (m) (:id m)) (list {:id 1 :name "a"} {:id 2 :name "b"})) 1)"# => common::eval(r#"{:id 1 :name "a"}"#),
    key_by_last_wins: r#"(:v (hashmap/get (list/key-by (fn (m) (:id m)) (list {:id 1 :v "x"} {:id 1 :v "y"})) 1))"# => Value::string("y"),
    key_by_empty: "(length (keys (list/key-by car '())))" => Value::int(0),
}

// ============================================================
// list/times
// ============================================================

eval_tests! {
    times_basic: "(list/times 5 (fn (i) (* i i)))" => common::eval("'(0 1 4 9 16)"),
    times_zero: "(list/times 0 (fn (i) i))" => common::eval("'()"),
    times_identity: "(list/times 3 (fn (i) i))" => common::eval("'(0 1 2)"),
}

// ============================================================
// list/duplicates
// ============================================================

eval_tests! {
    // NOTE: Depends on internal iteration order (likely insertion-order via seen set).
    // Wrapping in (sort ...) to make order-independence explicit.
    duplicates_basic: "(sort (list/duplicates '(1 2 2 3 3 3)))" => Value::list(vec![Value::int(2), Value::int(3)]),
    duplicates_none: "(list/duplicates '(1 2 3))" => Value::list(vec![]),
    duplicates_all: "(list/duplicates '(1 1 1))" => Value::list(vec![Value::int(1)]),
    duplicates_empty: "(list/duplicates '())" => Value::list(vec![]),
}

// ============================================================
// list/cross-join
// ============================================================

eval_tests! {
    cross_join_basic: "(list/cross-join '(1 2) '(3 4))" => common::eval("'((1 3) (1 4) (2 3) (2 4))"),
    cross_join_empty_first: "(list/cross-join '() '(1 2))" => common::eval("'()"),
    cross_join_empty_second: "(list/cross-join '(1 2) '())" => common::eval("'()"),
    cross_join_single: "(list/cross-join '(1) '(2))" => common::eval("'((1 2))"),
}

// ============================================================
// list/page
// ============================================================

eval_tests! {
    page_first: "(list/page '(1 2 3 4 5) 1 2)" => common::eval("'(1 2)"),
    page_second: "(list/page '(1 2 3 4 5) 2 2)" => common::eval("'(3 4)"),
    page_last_partial: "(list/page '(1 2 3 4 5) 3 2)" => common::eval("'(5)"),
    page_beyond: "(list/page '(1 2 3 4 5) 10 2)" => common::eval("'()"),
    page_empty: "(list/page '() 1 10)" => common::eval("'()"),
}

eval_error_tests! {
    page_zero: "(list/page '(1 2 3) 0 2)",
}

// ============================================================
// list/pad
// ============================================================

eval_tests! {
    pad_basic: "(list/pad '(1 2) 5 0)" => common::eval("'(1 2 0 0 0)"),
    pad_already_long: "(list/pad '(1 2 3) 2 0)" => common::eval("'(1 2 3)"),
    pad_exact: "(list/pad '(1 2 3) 3 0)" => common::eval("'(1 2 3)"),
    pad_empty: "(list/pad '() 3 0)" => common::eval("'(0 0 0)"),
    pad_string_fill: r#"(list/pad '() 2 "x")"# => common::eval(r#"'("x" "x")"#),
}

// ============================================================
// list/sole
// ============================================================

eval_tests! {
    sole_found: "(list/sole even? '(1 2 3))" => Value::int(2),
    sole_at_end: "(list/sole (fn (x) (> x 10)) '(1 5 20))" => Value::int(20),
}

eval_error_tests! {
    sole_none: "(list/sole even? '(1 3 5))",
    sole_multiple: "(list/sole even? '(2 4 6))",
}

// ============================================================
// list/join
// ============================================================

eval_tests! {
    join_basic: r#"(list/join '(1 2 3) ", ")"# => Value::string("1, 2, 3"),
    join_final_sep: r#"(list/join '(1 2 3) ", " " and ")"# => Value::string("1, 2 and 3"),
    join_single: r#"(list/join '(1) ", ")"# => Value::string("1"),
    join_two_final: r#"(list/join '(1 2) ", " " and ")"# => Value::string("1 and 2"),
    join_empty: r#"(list/join '() ", ")"# => Value::string(""),
    join_strings: r#"(list/join '("a" "b" "c") "-")"# => Value::string(r#""a"-"b"-"c""#),
}

// ============================================================
// rest / cdr
// ============================================================

eval_tests! {
    rest_list: "(rest '(1 2 3))" => Value::list(vec![Value::int(2), Value::int(3)]),
    rest_single: "(rest '(1))" => Value::list(vec![]),
    rest_vector: "(vector? (rest [1 2 3]))" => Value::bool(true),
    rest_vector_values: "(length (rest [1 2 3]))" => Value::int(2),
    cdr_list: "(cdr '(1 2 3))" => Value::list(vec![Value::int(2), Value::int(3)]),
}

// ============================================================
// vector
// ============================================================

eval_tests! {
    vector_create: "(vector? (vector 1 2 3))" => Value::bool(true),
    vector_length: "(length (vector 1 2 3))" => Value::int(3),
    vector_empty: "(vector? (vector))" => Value::bool(true),
    vector_nth: "(nth (vector 10 20 30) 1)" => Value::int(20),
}

// ============================================================
// car/cdr compositions (2-deep)
// ============================================================

eval_tests! {
    caar_basic: "(caar '((1 2) (3 4)))" => Value::int(1),
    cadr_basic: "(cadr '(1 2 3))" => Value::int(2),
    cdar_basic: "(cdar '((1 2 3) (4 5)))" => Value::list(vec![Value::int(2), Value::int(3)]),
    cddr_basic: "(cddr '(1 2 3 4))" => Value::list(vec![Value::int(3), Value::int(4)]),
}

// ============================================================
// car/cdr compositions (3-deep)
// ============================================================

eval_tests! {
    caaar_basic: "(caaar '(((1 2) 3) 4))" => Value::int(1),
    caadr_basic: "(caadr '(1 (2 3) 4))" => Value::int(2),
    cadar_basic: "(cadar '((1 2 3) 4))" => Value::int(2),
    caddr_basic: "(caddr '(1 2 3 4))" => Value::int(3),
    cdaar_basic: "(cdaar '(((1 2 3)) 4))" => Value::list(vec![Value::int(2), Value::int(3)]),
    cdadr_basic: "(cdadr '(1 (2 3 4)))" => Value::list(vec![Value::int(3), Value::int(4)]),
    cddar_basic: "(cddar '((1 2 3 4) 5))" => Value::list(vec![Value::int(3), Value::int(4)]),
    cdddr_basic: "(cdddr '(1 2 3 4 5))" => Value::list(vec![Value::int(4), Value::int(5)]),
}

// ============================================================
// assq / assv
// ============================================================

eval_tests! {
    assq_found: "(assq 2 '((1 10) (2 20) (3 30)))" => Value::list(vec![Value::int(2), Value::int(20)]),
    assq_missing: "(assq 5 '((1 10) (2 20)))" => Value::bool(false),
    assq_empty: "(assq 1 '())" => Value::bool(false),
    assq_first_match: "(car (cdr (assq 1 '((1 10) (1 99)))))" => Value::int(10),
    assv_found: "(assv 2 '((1 10) (2 20) (3 30)))" => Value::list(vec![Value::int(2), Value::int(20)]),
    assv_missing: "(assv 5 '((1 10) (2 20)))" => Value::bool(false),
}

// ============================================================
// frequencies
// ============================================================

eval_tests! {
    freq_basic: "(hashmap/get (frequencies '(1 1 2 3 3 3)) 3)" => Value::int(3),
    freq_single: "(hashmap/get (frequencies '(1 1 2 3 3 3)) 2)" => Value::int(1),
    freq_empty: "(length (keys (frequencies '())))" => Value::int(0),
    freq_all_same: "(hashmap/get (frequencies '(5 5 5)) 5)" => Value::int(3),
    freq_count_keys: "(length (keys (frequencies '(1 2 2 3 3 3))))" => Value::int(3),
}

// ============================================================
// list/shuffle, list/pick — random, test type/length only
// ============================================================

eval_tests! {
    shuffle_length: "(length (list/shuffle '(1 2 3)))" => Value::int(3),
    shuffle_empty: "(length (list/shuffle '()))" => Value::int(0),
    pick_is_number: "(number? (list/pick '(1 2 3)))" => Value::bool(true),
}

// ============================================================
// String error cases
// ============================================================

eval_error_tests! {
    str_ref_negative: r#"(string-ref "hello" -1)"#,
    str_ref_oob: r#"(string-ref "hello" 10)"#,
    substring_negative_start: r#"(substring "hello" -1 3)"#,
    substring_negative_end: r#"(substring "hello" 0 -1)"#,
    substring_start_gt_end: r#"(substring "hello" 3 1)"#,
    substring_oob: r#"(substring "hello" 0 10)"#,
}
