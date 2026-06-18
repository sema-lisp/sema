mod common;

use sema_core::Value;
use std::collections::BTreeMap;

// ============================================================
// hash-map constructor
// ============================================================

dual_eval_tests! {
    hash_map_empty: "(count (hash-map))" => Value::int(0),
    hash_map_basic: "(get (hash-map :x 1 :y 2) :x)" => Value::int(1),
    hash_map_basic_y: "(get (hash-map :x 1 :y 2) :y)" => Value::int(2),
}

dual_eval_error_tests! {
    hash_map_odd_args: "(hash-map :a 1 :b)",
}

// ============================================================
// get — maps and hashmaps
// ============================================================

dual_eval_tests! {
    get_map_found: "(get {:a 1 :b 2} :a)" => Value::int(1),
    get_map_missing: "(get {:a 1} :b)" => Value::nil(),
    get_map_default: "(get {:a 1} :b 99)" => Value::int(99),
    get_hashmap_found: "(get (hash-map :x 10) :x)" => Value::int(10),
    get_hashmap_missing: "(get (hash-map :x 10) :y)" => Value::nil(),
    get_hashmap_default: "(get (hash-map :x 10) :y 42)" => Value::int(42),
    get_keyword_shorthand: "(:a {:a 1 :b 2})" => Value::int(1),
}

dual_eval_error_tests! {
    get_bad_type: "(get 42 :a)",
}

// ============================================================
// assoc — Clojure-style map assoc + Scheme alist lookup
// ============================================================

dual_eval_tests! {
    assoc_map_add: "(get (assoc {:a 1} :b 2) :b)" => Value::int(2),
    assoc_map_overwrite: "(get (assoc {:a 1} :a 99) :a)" => Value::int(99),
    assoc_map_multi: "(count (assoc {:a 1} :b 2 :c 3))" => Value::int(3),
    assoc_hashmap_add: "(get (assoc (hash-map :a 1) :b 2) :b)" => Value::int(2),
    assoc_hashmap_overwrite: "(get (assoc (hash-map :a 1) :a 99) :a)" => Value::int(99),
    assoc_alist_found: "(assoc :a '((:a 1) (:b 2)))" => Value::list(vec![Value::keyword("a"), Value::int(1)]),
    assoc_alist_missing: "(assoc :z '((:a 1) (:b 2)))" => Value::bool(false),
}

dual_eval_error_tests! {
    assoc_bad_arity: "(assoc {:a 1} :b)",
    assoc_bad_type: "(assoc 42 :a 1)",
}

// ============================================================
// dissoc
// ============================================================

dual_eval_tests! {
    dissoc_map: "(contains? (dissoc {:a 1 :b 2} :a) :a)" => Value::bool(false),
    dissoc_map_preserves: "(get (dissoc {:a 1 :b 2} :a) :b)" => Value::int(2),
    dissoc_multi: "(count (dissoc {:a 1 :b 2 :c 3} :a :c))" => Value::int(1),
    dissoc_hashmap: "(contains? (dissoc (hash-map :a 1 :b 2) :a) :a)" => Value::bool(false),
    dissoc_hashmap_preserves: "(get (dissoc (hash-map :a 1 :b 2) :a) :b)" => Value::int(2),
}

dual_eval_error_tests! {
    dissoc_bad_type: "(dissoc 42 :a)",
}

// ============================================================
// keys / vals
// ============================================================

dual_eval_tests! {
    keys_map: "(sort (keys {:b 2 :a 1}))" => Value::list(vec![Value::keyword("a"), Value::keyword("b")]),
    keys_hashmap: "(length (keys (hash-map :a 1 :b 2)))" => Value::int(2),
    vals_map: "(sort (vals {:a 1 :b 2}))" => Value::list(vec![Value::int(1), Value::int(2)]),
    vals_hashmap: "(length (vals (hash-map :a 1 :b 2)))" => Value::int(2),
}

dual_eval_error_tests! {
    keys_bad_type: "(keys 42)",
    vals_bad_type: "(vals 42)",
}

// ============================================================
// merge
// ============================================================

dual_eval_tests! {
    merge_empty: "(count (merge))" => Value::int(0),
    merge_maps: "(get (merge {:a 1} {:b 2}) :b)" => Value::int(2),
    merge_overwrite: "(get (merge {:a 1} {:a 99}) :a)" => Value::int(99),
    merge_three: "(count (merge {:a 1} {:b 2} {:c 3}))" => Value::int(3),
    merge_hashmap_first: "(get (merge (hash-map :a 1) (hash-map :b 2)) :b)" => Value::int(2),
    merge_map_into_hashmap: "(get (merge (hash-map :a 1) {:b 2}) :b)" => Value::int(2),
    merge_hashmap_into_map: "(get (merge {:a 1} (hash-map :b 2)) :b)" => Value::int(2),
}

dual_eval_error_tests! {
    merge_bad_type: "(merge 42)",
    merge_bad_second: "(merge {:a 1} 42)",
}

// ============================================================
// contains? / count / empty?
// ============================================================

dual_eval_tests! {
    contains_map_yes: "(contains? {:a 1} :a)" => Value::bool(true),
    contains_map_no: "(contains? {:a 1} :b)" => Value::bool(false),
    contains_hashmap: "(contains? (hash-map :x 1) :x)" => Value::bool(true),
    count_map: "(count {:a 1 :b 2 :c 3})" => Value::int(3),
    count_hashmap: "(count (hash-map :a 1 :b 2))" => Value::int(2),
    count_list: "(count '(1 2 3))" => Value::int(3),
    count_vector: "(count [1 2 3])" => Value::int(3),
    count_string: "(count \"hello\")" => Value::int(5),
    count_nil: "(count nil)" => Value::int(0),
    empty_map: "(empty? {})" => Value::bool(true),
    empty_map_not: "(empty? {:a 1})" => Value::bool(false),
    empty_hashmap: "(empty? (hash-map))" => Value::bool(true),
    empty_hashmap_not: "(empty? (hash-map :a 1))" => Value::bool(false),
    empty_list: "(empty? '())" => Value::bool(true),
    empty_list_not: "(empty? '(1))" => Value::bool(false),
    empty_vector: "(empty? [])" => Value::bool(true),
    empty_string: "(empty? \"\")" => Value::bool(true),
    empty_string_not: "(empty? \"a\")" => Value::bool(false),
    empty_nil: "(empty? nil)" => Value::bool(true),
}

dual_eval_error_tests! {
    contains_bad_type: "(contains? 42 :a)",
    count_bad_type: "(count (fn (x) x))",
    empty_bad_type: "(empty? 42)",
}

// ============================================================
// map/entries
// ============================================================

dual_eval_tests! {
    entries_map: "(length (map/entries {:a 1 :b 2}))" => Value::int(2),
    entries_empty: "(map/entries {})" => Value::list(vec![]),
    entries_hashmap: "(length (map/entries (hash-map :a 1 :b 2)))" => Value::int(2),
}

dual_eval_error_tests! {
    entries_bad_type: "(map/entries 42)",
}

// ============================================================
// map/select-keys
// ============================================================

dual_eval_tests! {
    select_keys_map: "(count (map/select-keys {:a 1 :b 2 :c 3} '(:a :c)))" => Value::int(2),
    select_keys_missing: "(count (map/select-keys {:a 1} '(:x :y)))" => Value::int(0),
    select_keys_hashmap: "(count (map/select-keys (hash-map :a 1 :b 2 :c 3) '(:a :c)))" => Value::int(2),
    select_keys_vector: "(count (map/select-keys {:a 1 :b 2} [:a]))" => Value::int(1),
}

dual_eval_error_tests! {
    select_keys_bad_map: "(map/select-keys 42 '(:a))",
    select_keys_bad_keys: "(map/select-keys {:a 1} 42)",
}

// ============================================================
// map/map-keys / map/map-vals
// ============================================================

dual_eval_tests! {
    map_vals_basic: "(:a (map/map-vals (fn (v) (+ v 10)) {:a 1 :b 2}))" => Value::int(11),
    map_vals_hashmap: "(get (map/map-vals (fn (v) (* v 2)) (hash-map :x 5)) :x)" => Value::int(10),
    map_keys_basic: "(contains? (map/map-keys (fn (k) :z) {:a 1}) :z)" => Value::bool(true),
    map_keys_hashmap: "(contains? (map/map-keys (fn (k) :z) (hash-map :a 1)) :z)" => Value::bool(true),
}

dual_eval_error_tests! {
    map_vals_bad_type: "(map/map-vals (fn (v) v) 42)",
    map_keys_bad_type: "(map/map-keys (fn (k) k) 42)",
}

// ============================================================
// map/filter
// ============================================================

dual_eval_tests! {
    map_filter_basic: "(count (map/filter (fn (k v) (> v 1)) {:a 1 :b 2 :c 3}))" => Value::int(2),
    map_filter_none: "(count (map/filter (fn (k v) #f) {:a 1 :b 2}))" => Value::int(0),
    map_filter_hashmap: "(count (map/filter (fn (k v) (> v 1)) (hash-map :a 1 :b 2 :c 3)))" => Value::int(2),
}

dual_eval_error_tests! {
    map_filter_bad_type: "(map/filter (fn (k v) #t) 42)",
}

// ============================================================
// map/from-entries
// ============================================================

dual_eval_tests! {
    from_entries_basic: "(get (map/from-entries '((:a 1) (:b 2))) :a)" => Value::int(1),
    from_entries_vector: "(get (map/from-entries [[:a 1] [:b 2]]) :b)" => Value::int(2),
    from_entries_empty: "(count (map/from-entries '()))" => Value::int(0),
}

dual_eval_error_tests! {
    from_entries_bad_type: "(map/from-entries 42)",
    from_entries_bad_entry: "(map/from-entries '((:a 1 2)))",
    from_entries_non_pair: "(map/from-entries '(42))",
}

// ============================================================
// map/update
// ============================================================

dual_eval_tests! {
    map_update_basic: "(:a (map/update {:a 1} :a (fn (x) (+ x 10))))" => Value::int(11),
    map_update_missing: "(:b (map/update {:a 1} :b (fn (x) (if (nil? x) 99 x))))" => Value::int(99),
    map_update_hashmap: "(get (map/update (hash-map :a 5) :a (fn (x) (* x 2))) :a)" => Value::int(10),
}

dual_eval_error_tests! {
    map_update_bad_type: "(map/update 42 :a (fn (x) x))",
}

// ============================================================
// map/zip
// ============================================================

dual_eval_tests! {
    map_zip_basic: "(get (map/zip '(:a :b) '(1 2)) :a)" => Value::int(1),
    map_zip_vectors: "(get (map/zip [:a :b] [1 2]) :b)" => Value::int(2),
    map_zip_uneven: "(count (map/zip '(:a :b :c) '(1 2)))" => Value::int(2),
}

dual_eval_error_tests! {
    map_zip_bad_keys: "(map/zip 42 '(1))",
    map_zip_bad_vals: "(map/zip '(:a) 42)",
}

// ============================================================
// map/except
// ============================================================

dual_eval_tests! {
    map_except_basic: "(count (map/except {:a 1 :b 2 :c 3} '(:a :c)))" => Value::int(1),
    map_except_none: "(count (map/except {:a 1 :b 2} '(:x)))" => Value::int(2),
    map_except_hashmap: "(count (map/except (hash-map :a 1 :b 2 :c 3) '(:b)))" => Value::int(2),
    map_except_vector: "(count (map/except {:a 1 :b 2} [:a]))" => Value::int(1),
}

dual_eval_error_tests! {
    map_except_bad_map: "(map/except 42 '(:a))",
    map_except_bad_keys: "(map/except {:a 1} 42)",
}

// ============================================================
// map/sort-keys
// ============================================================

dual_eval_tests! {
    sort_keys_map: "(count (map/sort-keys {:b 2 :a 1}))" => Value::int(2),
    sort_keys_hashmap: "(count (map/sort-keys (hash-map :b 2 :a 1)))" => Value::int(2),
}

dual_eval_error_tests! {
    sort_keys_bad_type: "(map/sort-keys 42)",
}

// ============================================================
// hashmap/* namespace functions
// ============================================================

dual_eval_tests! {
    hashmap_new_empty: "(count (hashmap/new))" => Value::int(0),
    hashmap_new_basic: "(get (hashmap/new :a 1 :b 2) :a)" => Value::int(1),
    hashmap_get_found: "(hashmap/get (hash-map :a 1) :a)" => Value::int(1),
    hashmap_get_missing: "(hashmap/get (hash-map :a 1) :b)" => Value::nil(),
    hashmap_get_default: "(hashmap/get (hash-map :a 1) :b 99)" => Value::int(99),
    hashmap_get_from_sorted: "(hashmap/get {:a 1} :a)" => Value::int(1),
    hashmap_assoc_add: "(get (hashmap/assoc (hashmap/new :a 1) :b 2) :b)" => Value::int(2),
    hashmap_assoc_multi: "(count (hashmap/assoc (hashmap/new :a 1) :b 2 :c 3))" => Value::int(3),
    hashmap_to_map: "(count (hashmap/to-map (hashmap/new :a 1 :b 2)))" => Value::int(2),
    hashmap_keys_count: "(length (hashmap/keys (hashmap/new :a 1 :b 2)))" => Value::int(2),
    hashmap_contains_yes: "(hashmap/contains? (hashmap/new :a 1) :a)" => Value::bool(true),
    hashmap_contains_no: "(hashmap/contains? (hashmap/new :a 1) :b)" => Value::bool(false),
}

dual_eval_error_tests! {
    hashmap_new_odd: "(hashmap/new :a)",
    hashmap_get_bad_type: "(hashmap/get 42 :a)",
    hashmap_assoc_bad_arity: "(hashmap/assoc (hash-map :a 1) :b)",
    hashmap_assoc_bad_type: "(hashmap/assoc 42 :a 1)",
    hashmap_to_map_bad_type: "(hashmap/to-map 42)",
    hashmap_keys_bad_type: "(hashmap/keys 42)",
    hashmap_contains_bad_type: "(hashmap/contains? 42 :a)",
}

// ============================================================
// get-in — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    get_in_basic: r#"(get-in {:a {:b {:c 42}}} [:a :b :c])"# => Value::int(42),
    get_in_missing_nil: r#"(get-in {:a {:b 1}} [:a :c])"# => Value::nil(),
    get_in_missing_default: r#"(get-in {:a {:b 1}} [:a :c] "default")"# => Value::string("default"),
    get_in_nil_intermediate: r#"(get-in {:a nil} [:a :b :c])"# => Value::nil(),
    get_in_empty_path: r#"(get-in {:a 1} [])"# => Value::map(BTreeMap::from([(Value::keyword("a"), Value::int(1))])),
    get_in_non_map_intermediate: r#"(get-in {:a 42} [:a :b] "default")"# => Value::string("default"),
}

dual_eval_error_tests! {
    get_in_bad_path: "(get-in {:a 1} 42)",
}

// ============================================================
// assoc-in — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    assoc_in_basic: r#"(get-in (assoc-in {:a {:b 1}} [:a :b] 42) [:a :b])"# => Value::int(42),
    assoc_in_creates_nested: r#"(get-in (assoc-in {} [:a :b :c] 99) [:a :b :c])"# => Value::int(99),
    assoc_in_empty_path: r#"(assoc-in {:a 1} [] 42)"# => Value::int(42),
}

// ============================================================
// update-in — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    update_in_basic: r#"(get-in (update-in {:a {:b 10}} [:a :b] (fn (x) (+ x 1))) [:a :b])"# => Value::int(11),
    update_in_missing: r#"(get-in (update-in {} [:a :b] (fn (x) (if (nil? x) 1 (+ x 1)))) [:a :b])"# => Value::int(1),
    update_in_empty_path: r#"(update-in 5 [] (fn (x) (+ x 1)))"# => Value::int(6),
}

// ============================================================
// deep-merge — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    deep_merge_empty: "(count (deep-merge))" => Value::int(0),
    deep_merge_preserves: r#"(get-in (deep-merge {:a {:b 1 :c 2}} {:a {:b 99}}) [:a :c])"# => Value::int(2),
    deep_merge_overwrites: r#"(get-in (deep-merge {:a {:b 1 :c 2}} {:a {:b 99}}) [:a :b])"# => Value::int(99),
    deep_merge_non_map: r#"(:a (deep-merge {:a {:b 1}} {:a 42}))"# => Value::int(42),
    deep_merge_multiple: r#"(get-in (deep-merge {:a 1} {:b 2} {:c 3}) [:c])"# => Value::int(3),
    deep_merge_single: r#"(:a (deep-merge {:a 1}))"# => Value::int(1),
}

// ============================================================
// Aliases
// ============================================================

dual_eval_tests! {
    alias_hash_ref: "(hash-ref {:a 42} :a)" => Value::int(42),
}
