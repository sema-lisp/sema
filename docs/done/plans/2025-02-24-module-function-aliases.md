# Module/Function Aliases for Legacy Scheme Names — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `module/function` style aliases for legacy Scheme-named builtins that are clearly module-specific. Generic/polymorphic functions (`map`, `filter`, `length`, `append`, etc.) stay un-namespaced. Legacy names keep working.

**Architecture:** For each legacy function, add a slash-style alias using `env.set()` at the bottom of each `register()` function, guarded by `env.get(alias).is_none()` to prevent overwrites. Only alias functions that are unambiguously module-specific — not generic collection operations.

**Tech Stack:** Rust (sema-stdlib crate), dual-eval integration tests

---

## Design Principles

1. **Only namespace module-specific functions.** If a function works on multiple types (list, vector, string, map), it stays global.
2. **Guard against overwrites.** Use `if env.get(intern("alias")).is_none()` before setting.
3. **Arrow conversions use `X/to-Y` pattern.** e.g. `string->symbol` → `string/to-symbol`.
4. **Legacy names remain working.** They become silent aliases pointing to the same value.

### What gets aliased vs what stays global

| Gets `module/` alias | Stays global (polymorphic) |
|---|---|
| `string-length`, `string-append`, `string-ref` | `map`, `filter`, `foldl`, `for-each` |
| `char->integer`, `char-alphabetic?` | `length`, `count`, `append`, `reverse` |
| `make-bytevector`, `bytevector-length` | `sort`, `flatten`, `take`, `drop`, `zip` |
| `hash-map`, `get-in`, `assoc-in` | `range`, `any`, `every`, `nth` |
| `read-line`, `print-error` | `assoc`, `merge`, `contains?`, `empty?` |
| `deep-merge` | `cons`, `get`, `keys`, `vals`, `dissoc` |

---

## Task 1: String & Char Module Aliases (`crates/sema-stdlib/src/string.rs`)

**Files:**
- Modify: `crates/sema-stdlib/src/string.rs` (alias block at ~L1214)
- Test: `crates/sema/tests/dual_eval_test.rs`

### Aliases to add:

| Legacy Name | New Alias |
|---|---|
| `string-append` | `string/append` |
| `string-length` | `string/length` |
| `string-ref` | `string/ref` |
| `substring` | `string/slice` |
| `string->symbol` | `string/to-symbol` |
| `symbol->string` | `symbol/to-string` |
| `string->keyword` | `string/to-keyword` |
| `keyword->string` | `keyword/to-string` |
| `number->string` | `number/to-string` |
| `string->number` | `string/to-number` |
| `string->float` | `string/to-float` |
| `char->integer` | `char/to-integer` |
| `integer->char` | `integer/to-char` |
| `char->string` | `char/to-string` |
| `string->char` | `string/to-char` |
| `string->list` | `string/to-list` |
| `char-alphabetic?` | `char/alphabetic?` |
| `char-numeric?` | `char/numeric?` |
| `char-whitespace?` | `char/whitespace?` |
| `char-upper-case?` | `char/upper-case?` |
| `char-lower-case?` | `char/lower-case?` |
| `char-upcase` | `char/upcase` |
| `char-downcase` | `char/downcase` |

**Step 1: Write dual-eval tests**

Add to `crates/sema/tests/dual_eval_test.rs`:

```rust
dual_eval_tests! {
    string_length_alias: r#"(string/length "hello")"# => "5",
    string_append_alias: r#"(string/append "a" "b")"# => "\"ab\"",
    string_ref_alias: r#"(string/ref "hello" 0)"# => "#\\h",
    string_slice_alias: r#"(string/slice "hello" 1 3)"# => "\"el\"",
    string_to_symbol_alias: r#"(string/to-symbol "foo")"# => "foo",
    symbol_to_string_alias: r#"(symbol/to-string 'foo)"# => "\"foo\"",
    string_to_number_alias: r#"(string/to-number "42")"# => "42",
    number_to_string_alias: r#"(number/to-string 42)"# => "\"42\"",
    string_to_keyword_alias: r#"(keyword? (string/to-keyword "foo"))"# => "#t",
    keyword_to_string_alias: r#"(keyword/to-string :foo)"# => "\"foo\"",
    char_to_integer_alias: r#"(char/to-integer #\a)"# => "97",
    integer_to_char_alias: r#"(integer/to-char 97)"# => "#\\a",
    char_to_string_alias: r#"(char/to-string #\a)"# => "\"a\"",
    string_to_char_alias: r#"(string/to-char "a")"# => "#\\a",
    string_to_list_alias: r#"(length (string/to-list "abc"))"# => "3",
    char_alphabetic_alias: r#"(char/alphabetic? #\a)"# => "#t",
    char_numeric_alias: r#"(char/numeric? #\5)"# => "#t",
    char_whitespace_alias: r#"(char/whitespace? #\space)"# => "#t",
    char_upcase_alias: r#"(char/upcase #\a)"# => "#\\A",
    char_downcase_alias: r#"(char/downcase #\A)"# => "#\\a",
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p sema --test dual_eval_test -- string_length_alias`
Expected: FAIL (function not found)

**Step 3: Add aliases to `string.rs`**

Append to the existing alias block at the bottom of `register()` (after L1232):

```rust
    // module/function aliases for legacy Scheme names
    if let Some(v) = env.get(sema_core::intern("string-append")) {
        env.set(sema_core::intern("string/append"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string-length")) {
        env.set(sema_core::intern("string/length"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string-ref")) {
        env.set(sema_core::intern("string/ref"), v);
    }
    if let Some(v) = env.get(sema_core::intern("substring")) {
        env.set(sema_core::intern("string/slice"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->symbol")) {
        env.set(sema_core::intern("string/to-symbol"), v);
    }
    if let Some(v) = env.get(sema_core::intern("symbol->string")) {
        env.set(sema_core::intern("symbol/to-string"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->keyword")) {
        env.set(sema_core::intern("string/to-keyword"), v);
    }
    if let Some(v) = env.get(sema_core::intern("keyword->string")) {
        env.set(sema_core::intern("keyword/to-string"), v);
    }
    if let Some(v) = env.get(sema_core::intern("number->string")) {
        env.set(sema_core::intern("number/to-string"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->number")) {
        env.set(sema_core::intern("string/to-number"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->float")) {
        env.set(sema_core::intern("string/to-float"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char->integer")) {
        env.set(sema_core::intern("char/to-integer"), v);
    }
    if let Some(v) = env.get(sema_core::intern("integer->char")) {
        env.set(sema_core::intern("integer/to-char"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char->string")) {
        env.set(sema_core::intern("char/to-string"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->char")) {
        env.set(sema_core::intern("string/to-char"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->list")) {
        env.set(sema_core::intern("string/to-list"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-alphabetic?")) {
        env.set(sema_core::intern("char/alphabetic?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-numeric?")) {
        env.set(sema_core::intern("char/numeric?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-whitespace?")) {
        env.set(sema_core::intern("char/whitespace?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-upper-case?")) {
        env.set(sema_core::intern("char/upper-case?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-lower-case?")) {
        env.set(sema_core::intern("char/lower-case?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-upcase")) {
        env.set(sema_core::intern("char/upcase"), v);
    }
    if let Some(v) = env.get(sema_core::intern("char-downcase")) {
        env.set(sema_core::intern("char/downcase"), v);
    }
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema --test dual_eval_test -- alias`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add crates/sema-stdlib/src/string.rs crates/sema/tests/dual_eval_test.rs
git commit -m "feat: add module/function aliases for string and char builtins"
```

---

## Task 2: Map Module Aliases (`crates/sema-stdlib/src/map.rs`)

**Files:**
- Modify: `crates/sema-stdlib/src/map.rs` (alias block at ~L750)
- Test: `crates/sema/tests/dual_eval_test.rs`

Only alias map-specific functions. Generic collection functions (`get`, `keys`, `vals`, `merge`, `assoc`, `dissoc`, `contains?`, `count`, `empty?`) stay global.

### Aliases to add:

| Legacy Name | New Alias |
|---|---|
| `hash-map` | `map/new` |
| `deep-merge` | `map/deep-merge` |
| `get-in` | `map/get-in` |
| `assoc-in` | `map/assoc-in` |
| `update-in` | `map/update-in` |

**Step 1: Write dual-eval tests**

```rust
dual_eval_tests! {
    map_new_alias: r#"(map? (map/new :a 1))"# => "#t",
    map_deep_merge_alias: r#"(get (map/deep-merge {:a 1} {:b 2}) :b)"# => "2",
    map_get_in_alias: r#"(map/get-in {:a {:b 42}} '(:a :b))"# => "42",
    map_assoc_in_alias: r#"(map/get-in (map/assoc-in {} '(:a :b) 1) '(:a :b))"# => "1",
}
```

**Step 2: Run tests, verify failure**

**Step 3: Add aliases to `map.rs`**

Append to existing alias block at bottom of `register()`:

```rust
    // module/function aliases for map-specific operations
    if let Some(v) = env.get(sema_core::intern("hash-map")) {
        env.set(sema_core::intern("map/new"), v);
    }
    if let Some(v) = env.get(sema_core::intern("deep-merge")) {
        env.set(sema_core::intern("map/deep-merge"), v);
    }
    if let Some(v) = env.get(sema_core::intern("get-in")) {
        env.set(sema_core::intern("map/get-in"), v);
    }
    if let Some(v) = env.get(sema_core::intern("assoc-in")) {
        env.set(sema_core::intern("map/assoc-in"), v);
    }
    if let Some(v) = env.get(sema_core::intern("update-in")) {
        env.set(sema_core::intern("map/update-in"), v);
    }
```

**Step 4: Run tests**

Run: `cargo test -p sema --test dual_eval_test -- map_`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add crates/sema-stdlib/src/map.rs crates/sema/tests/dual_eval_test.rs
git commit -m "feat: add module/function aliases for map builtins"
```

---

## Task 3: Bytevector Module Aliases (`crates/sema-stdlib/src/bytevector.rs`)

**Files:**
- Modify: `crates/sema-stdlib/src/bytevector.rs`
- Test: `crates/sema/tests/dual_eval_test.rs`

### Aliases to add:

| Legacy Name | New Alias |
|---|---|
| `make-bytevector` | `bytevector/new` |
| `bytevector-length` | `bytevector/length` |
| `bytevector-u8-ref` | `bytevector/ref` |
| `bytevector-u8-set!` | `bytevector/set!` |
| `bytevector-copy` | `bytevector/copy` |
| `bytevector-append` | `bytevector/append` |
| `bytevector->list` | `bytevector/to-list` |
| `list->bytevector` | `list/to-bytevector` |
| `string->utf8` | `string/to-utf8` |
| `utf8->string` | `utf8/to-string` |

**Step 1: Write dual-eval tests**

```rust
dual_eval_tests! {
    bytevector_new_alias: r#"(bytevector/length (bytevector/new 3))"# => "3",
    bytevector_length_alias: r#"(bytevector/length (bytevector 1 2 3))"# => "3",
    bytevector_ref_alias: r#"(bytevector/ref (bytevector 10 20 30) 1)"# => "20",
    bytevector_append_alias: r#"(bytevector/length (bytevector/append (bytevector 1) (bytevector 2)))"# => "2",
    bytevector_to_list_alias: r#"(length (bytevector/to-list (bytevector 1 2 3)))"# => "3",
    string_to_utf8_alias: r#"(bytevector/length (string/to-utf8 "hi"))"# => "2",
}
```

**Step 2–4:** Same pattern: run fail → add aliases at bottom of `register()` → run pass.

```rust
    // module/function aliases for legacy Scheme names
    if let Some(v) = env.get(sema_core::intern("make-bytevector")) {
        env.set(sema_core::intern("bytevector/new"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector-length")) {
        env.set(sema_core::intern("bytevector/length"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector-u8-ref")) {
        env.set(sema_core::intern("bytevector/ref"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector-u8-set!")) {
        env.set(sema_core::intern("bytevector/set!"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector-copy")) {
        env.set(sema_core::intern("bytevector/copy"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector-append")) {
        env.set(sema_core::intern("bytevector/append"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector->list")) {
        env.set(sema_core::intern("bytevector/to-list"), v);
    }
    if let Some(v) = env.get(sema_core::intern("list->bytevector")) {
        env.set(sema_core::intern("list/to-bytevector"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->utf8")) {
        env.set(sema_core::intern("string/to-utf8"), v);
    }
    if let Some(v) = env.get(sema_core::intern("utf8->string")) {
        env.set(sema_core::intern("utf8/to-string"), v);
    }
```

**Step 5: Commit**

```bash
git add crates/sema-stdlib/src/bytevector.rs crates/sema/tests/dual_eval_test.rs
git commit -m "feat: add module/function aliases for bytevector builtins"
```

---

## Task 4: IO Module Aliases (`crates/sema-stdlib/src/io.rs`)

**Files:**
- Modify: `crates/sema-stdlib/src/io.rs`

### Aliases to add:

| Legacy Name | New Alias |
|---|---|
| `read-line` | `io/read-line` |
| `read-many` | `io/read-many` |
| `read-stdin` | `io/read-stdin` |
| `print-error` | `io/print-error` |
| `println-error` | `io/println-error` |

**Step 1: Add aliases at bottom of `register()` in `io.rs`**

```rust
    // module/function aliases
    if let Some(v) = env.get(sema_core::intern("read-line")) {
        env.set(sema_core::intern("io/read-line"), v);
    }
    if let Some(v) = env.get(sema_core::intern("read-many")) {
        env.set(sema_core::intern("io/read-many"), v);
    }
    if let Some(v) = env.get(sema_core::intern("read-stdin")) {
        env.set(sema_core::intern("io/read-stdin"), v);
    }
    if let Some(v) = env.get(sema_core::intern("print-error")) {
        env.set(sema_core::intern("io/print-error"), v);
    }
    if let Some(v) = env.get(sema_core::intern("println-error")) {
        env.set(sema_core::intern("io/println-error"), v);
    }
```

**Step 2: Commit**

```bash
git add crates/sema-stdlib/src/io.rs
git commit -m "feat: add module/function aliases for io builtins"
```

---

## Task 5: Full Test Suite Verification

**Step 1: Run all tests**

Run: `cargo test`
Expected: ALL PASS, no regressions

**Step 2: Run clippy**

Run: `make lint`
Expected: No warnings

**Step 3: Final commit if any fixups needed**

---

## Task 6: Update Playground Examples (separate follow-up)

Audit `playground/examples/` and `examples/` for legacy names and update to use the new canonical slash-style names. Rebuild playground examples:

```bash
cd playground && node build.mjs
```

This task is lower priority and can be done as a follow-up PR.

---

## Summary of All New Aliases

| Count | Module | Examples |
|---|---|---|
| 23 | string/char | `string/length`, `string/append`, `char/to-integer`, etc. |
| 5 | map | `map/new`, `map/deep-merge`, `map/get-in`, etc. |
| 10 | bytevector | `bytevector/length`, `bytevector/ref`, etc. |
| 5 | io | `io/read-line`, `io/read-stdin`, etc. |
| **43** | **Total** | |

### NOT aliased (generic/polymorphic — stay global)

`map`, `filter`, `foldl`, `for-each`, `length`, `count`, `append`, `reverse`, `sort`, `flatten`, `take`, `drop`, `zip`, `range`, `any`, `every`, `nth`, `cons`, `get`, `keys`, `vals`, `merge`, `assoc`, `dissoc`, `contains?`, `empty?`
