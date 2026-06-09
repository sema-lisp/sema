# Auto-Gensym (`foo#`) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add Clojure-style auto-gensym syntax where symbols ending in `#` inside quasiquote templates are automatically replaced with unique gensym'd symbols, preventing variable capture in macros.

**Architecture:** The `#` suffix is allowed in symbols at the lexer level. During quasiquote expansion (both tree-walker and VM), any symbol ending with `#` is detected and replaced with a unique gensym (`<prefix>__<counter>`). All occurrences of the same `foo#` within a single quasiquote form map to the same gensym. Each new quasiquote evaluation generates fresh gensyms. Outside quasiquote, `foo#` is just a regular symbol with no special behavior.

**Tech Stack:** Rust 2021, sema-reader (lexer), sema-core (shared gensym counter), sema-eval (tree-walker quasiquote), sema-vm (VM quasiquote lowering)

**Design decisions:**
- A single gensym counter lives in `sema-core` (shared by manual `gensym`, tree-walker auto-gensym, and VM auto-gensym) to prevent counter collisions producing identical symbols.
- Only a single trailing `#` triggers auto-gensym (`x#` yes, `x##` no). This avoids ambiguity.
- Nested quasiquote is out of scope — Sema doesn't support it correctly today, and auto-gensym inherits that limitation.
- VM bakes auto-gensyms at compile time; tree-walker resolves them at runtime. For macros (the primary use case) this is equivalent since both expand macros before evaluation.

---

## Task 1: Move gensym counter to `sema-core`

The gensym counter currently lives in `sema-stdlib/src/meta.rs`. Both the tree-walker quasiquote and VM lowering need to generate unique symbols. To prevent collisions between manual `(gensym)` and auto-gensym `foo#`, all three must share a single counter.

**Files:**
- Modify: `crates/sema-core/src/value.rs` (add `next_gensym` function near the `intern` function)
- Modify: `crates/sema-stdlib/src/meta.rs` (use shared counter)

**Step 1: Add shared gensym counter to `sema-core`**

In `crates/sema-core/src/value.rs`, after the existing `intern`/`resolve` functions, add:

```rust
// ── Gensym counter ────────────────────────────────────────────────

thread_local! {
    static GENSYM_COUNTER: Cell<u64> = const { Cell::new(0) };
}

/// Generate a unique symbol name: `<prefix>__<counter>`.
/// Used by both manual `(gensym)` and auto-gensym `foo#` in quasiquote.
pub fn next_gensym(prefix: &str) -> String {
    GENSYM_COUNTER.with(|c| {
        let val = c.get();
        c.set(val + 1);
        format!("{prefix}__{val}")
    })
}
```

Add `use std::cell::Cell;` to imports if not already present.

Also export `next_gensym` from `crates/sema-core/src/lib.rs`.

**Step 2: Update `sema-stdlib/src/meta.rs` to use the shared counter**

Replace the local `GENSYM_COUNTER` thread-local and inline counter logic with a call to `sema_core::next_gensym`:

```rust
register_fn(env, "gensym", |args| {
    check_arity!(args, "gensym", 0..=1);
    let prefix = if args.len() == 1 {
        args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string()
    } else {
        "g".to_string()
    };
    Ok(Value::symbol(&sema_core::next_gensym(&prefix)))
});
```

Remove the local `GENSYM_COUNTER` thread-local and the `use std::cell::Cell;` import (if no longer needed).

**Step 3: Run tests**

Run: `cargo test -p sema-stdlib`
Run: `cargo test -p sema -- gensym`
Expected: All pass — behavior is identical, just the counter location moved.

**Step 4: Commit**

```bash
git add crates/sema-core/src/value.rs crates/sema-core/src/lib.rs crates/sema-stdlib/src/meta.rs
git commit -m "refactor: move gensym counter to sema-core for shared use"
```

---

## Task 2: Allow `#` as a trailing character in symbols

The lexer currently does not include `#` in `is_symbol_char`. We need to allow it so that `v#`, `tmp#`, `old#` parse as valid symbols.

**Files:**
- Modify: `crates/sema-reader/src/lexer.rs:645-647` (`is_symbol_char`)

**Step 1: Write failing reader test**

Add to the reader unit tests in `crates/sema-reader/src/reader.rs` (at the end of the test module):

```rust
#[test]
fn test_auto_gensym_symbol_parsing() {
    // Symbols ending in # should parse successfully
    let val = read("v#").unwrap();
    assert_eq!(val.as_symbol().unwrap(), "v#");

    let val = read("tmp#").unwrap();
    assert_eq!(val.as_symbol().unwrap(), "tmp#");

    // Should work inside quasiquote
    let val = read("`(let ((v# 1)) v#)").unwrap();
    let items = val.as_list().unwrap();
    assert_eq!(items[0].as_symbol().unwrap(), "quasiquote");
}

#[test]
fn test_hash_reader_dispatch_still_works() {
    // Confirm # reader dispatch is not broken by allowing # in symbols
    let val = read("#t").unwrap();
    assert_eq!(val.as_bool(), Some(true));

    let val = read("#f").unwrap();
    assert_eq!(val.as_bool(), Some(false));

    let val = read("#\\space").unwrap();
    assert_eq!(val.as_char(), Some(' '));

    // #( short lambda still parses
    let val = read("#(+ % 1)").unwrap();
    assert!(val.as_list().is_some());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p sema-reader -- test_auto_gensym_symbol_parsing`
Expected: FAIL — `#` is not recognized as a symbol character, so `v#` will parse `v` then fail on `#`.

Run: `cargo test -p sema-reader -- test_hash_reader_dispatch_still_works`
Expected: PASS — these should already work (regression baseline).

**Step 3: Add `#` to `is_symbol_char`**

In `crates/sema-reader/src/lexer.rs`, modify `is_symbol_char`:

```rust
fn is_symbol_char(ch: char) -> bool {
    is_symbol_start(ch) || ch.is_ascii_digit() || matches!(ch, '-' | '/' | '.' | '#')
}
```

Note: `#` must NOT be added to `is_symbol_start` — a symbol cannot start with `#` (that's the reader dispatch character for `#t`, `#f`, `#(`, `#"`, `#\`, `#u8(`). It can only appear mid/end of symbol.

**Step 4: Run tests to verify both pass**

Run: `cargo test -p sema-reader -- test_auto_gensym_symbol_parsing`
Expected: PASS

Run: `cargo test -p sema-reader -- test_hash_reader_dispatch_still_works`
Expected: PASS (no regressions)

**Step 5: Commit**

```bash
git add crates/sema-reader/src/lexer.rs crates/sema-reader/src/reader.rs
git commit -m "feat(reader): allow # as trailing character in symbols for auto-gensym"
```

---

## Task 3: Auto-gensym in tree-walker quasiquote expansion

The tree-walker's `expand_quasiquote` in `special_forms.rs` must detect symbols ending with a single `#` and replace them with consistent gensyms within a single quasiquote form.

**Files:**
- Modify: `crates/sema-eval/src/special_forms.rs:861-920` (`eval_quasiquote` and `expand_quasiquote`)

**Step 1: Write failing dual-eval test (tree-walker half will run)**

Add a new section to `crates/sema/tests/dual_eval_test.rs`:

```rust
// ============================================================
// Auto-gensym — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    // Basic: auto-gensym in quasiquote produces a unique symbol that doesn't capture
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
    "# => common::eval_tw("[5 5]"),

    // x## (double hash) is NOT auto-gensym — only single trailing # triggers it
    auto_gensym_double_hash_is_regular: r#"
        (begin
          (define x## 99)
          x##)
    "# => Value::int(99),
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p sema -- auto_gensym`
Expected: FAIL — `my-let1` expands `x#` literally, so `(let ((x# 42)) x)` would either error or not shadow `x`.

**Step 3: Implement auto-gensym in tree-walker quasiquote**

In `crates/sema-eval/src/special_forms.rs`, modify `eval_quasiquote` and `expand_quasiquote`:

1. Add `use std::collections::HashMap;` to imports if not present.

2. Add a helper to detect auto-gensym symbols (single trailing `#`, not `##`):

```rust
/// Check if a symbol is an auto-gensym candidate: ends with exactly one `#`.
fn is_auto_gensym(sym: &str) -> bool {
    sym.len() > 1 && sym.ends_with('#') && !sym.ends_with("##")
}
```

3. Change `eval_quasiquote` to create a fresh gensym mapping and pass it down:

```rust
fn eval_quasiquote(args: &[Value], env: &Env, ctx: &EvalContext) -> Result<Trampoline, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("quasiquote", "1", args.len()));
    }
    let mut gensym_map: HashMap<String, String> = HashMap::new();
    let result = expand_quasiquote(&args[0], env, ctx, &mut gensym_map)?;
    Ok(Trampoline::Value(result))
}
```

4. Update `expand_quasiquote` to accept `&mut HashMap<String, String>` and handle `#`-suffixed symbols. Uses `sema_core::next_gensym` (shared counter from Task 1):

```rust
fn expand_quasiquote(
    val: &Value,
    env: &Env,
    ctx: &EvalContext,
    gensym_map: &mut HashMap<String, String>,
) -> Result<Value, SemaError> {
    // Check for auto-gensym symbol (foo#)
    if let Some(sym) = val.as_symbol() {
        if is_auto_gensym(sym) {
            let prefix = &sym[..sym.len() - 1];
            let resolved = gensym_map
                .entry(sym.to_string())
                .or_insert_with(|| sema_core::next_gensym(prefix))
                .clone();
            return Ok(Value::symbol(&resolved));
        }
    }

    if let Some(items) = val.as_list() {
        // ... rest of existing logic, passing gensym_map to recursive calls ...
    }
    // ... rest unchanged, passing gensym_map to recursive calls ...
}
```

All recursive calls to `expand_quasiquote` within the function must pass `gensym_map` through.

**Step 4: Run tree-walker tests to verify they pass**

Run: `cargo test -p sema -- auto_gensym_tw`
Expected: PASS for all `_tw` variants

**Step 5: Commit**

```bash
git add crates/sema-eval/src/special_forms.rs crates/sema/tests/dual_eval_test.rs
git commit -m "feat(eval): auto-gensym in tree-walker quasiquote expansion"
```

---

## Task 4: Auto-gensym in VM quasiquote lowering

The VM's `expand_quasiquote` in `lower.rs` must perform the same `foo#` → gensym replacement at compile time. Uses `sema_core::next_gensym` (shared counter from Task 1).

**Files:**
- Modify: `crates/sema-vm/src/lower.rs:822-923` (`lower_quasiquote` and `expand_quasiquote`)

**Step 1: Verify `_vm` tests fail**

Run: `cargo test -p sema -- auto_gensym_vm`
Expected: FAIL — VM's quasiquote doesn't handle auto-gensym yet.

**Step 2: Implement auto-gensym in VM quasiquote lowering**

In `crates/sema-vm/src/lower.rs`:

1. Add `use std::collections::HashMap;` to imports if not present.

2. Add the same `is_auto_gensym` helper (or move it to `sema-core` to share — but since it's a 3-line function, duplicating is fine):

```rust
fn is_auto_gensym(sym: &str) -> bool {
    sym.len() > 1 && sym.ends_with('#') && !sym.ends_with("##")
}
```

3. Change `lower_quasiquote` to create a gensym map:

```rust
fn lower_quasiquote(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("quasiquote", "1", args.len()));
    }
    let mut gensym_map: HashMap<String, String> = HashMap::new();
    expand_quasiquote(&args[0], &mut gensym_map)
}
```

4. Update `expand_quasiquote` signature and add auto-gensym symbol handling:

```rust
fn expand_quasiquote(val: &Value, gensym_map: &mut HashMap<String, String>) -> Result<CoreExpr, SemaError> {
    // Check for auto-gensym symbol (foo#)
    if let Some(sym) = val.as_symbol() {
        if is_auto_gensym(sym) {
            let prefix = &sym[..sym.len() - 1];
            let resolved = gensym_map
                .entry(sym.to_string())
                .or_insert_with(|| sema_core::next_gensym(prefix))
                .clone();
            return Ok(CoreExpr::Quote(Value::symbol(&resolved)));
        }
    }

    match val.view() {
        // ... existing match arms, passing gensym_map to recursive calls ...
    }
}
```

All recursive calls to `expand_quasiquote` within the function must pass `gensym_map` through.

**Important:** The VM version returns `CoreExpr::Quote(Value::symbol(...))` (not `CoreExpr::Var`), because the gensym'd symbol is being used as *data* in the quasiquote template, not as a variable reference. The surrounding `let`/`define` forms that bind this symbol will handle the variable semantics.

**Step 3: Run all auto-gensym tests**

Run: `cargo test -p sema -- auto_gensym`
Expected: PASS for all `_tw` and `_vm` variants

**Step 4: Commit**

```bash
git add crates/sema-vm/src/lower.rs
git commit -m "feat(vm): auto-gensym in VM quasiquote lowering"
```

---

## Task 5: Update prelude macros to use auto-gensym

Replace the hardcoded `__v` in `some->` with `v#` to demonstrate the feature and fix the existing hygiene bug.

**Files:**
- Modify: `crates/sema-eval/src/prelude.rs`

**Step 1: Write a test that demonstrates the current bug**

Add to `crates/sema/tests/dual_eval_test.rs`:

```rust
dual_eval_tests! {
    // some-> should not capture user's variables
    some_arrow_no_capture: r#"
        (begin
          (define __v {:name "Alice" :age 30})
          (some-> __v (:name)))
    "# => Value::string("Alice"),
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p sema -- some_arrow_no_capture`
Expected: FAIL — `some->` uses `__v` internally, which captures the user's `__v`.

**Step 3: Update `some->` to use `v#`**

In `crates/sema-eval/src/prelude.rs`, replace `__v` with `v#`:

```rust
(defmacro some-> (val . forms)
  (if (null? forms)
    val
    (let ((form (car forms))
          (rest (cdr forms)))
      (if (list? form)
        `(let ((v# ,val))
           (if (nil? v#) nil (some-> (,(car form) v# ,@(cdr form)) ,@rest)))
        `(let ((v# ,val))
           (if (nil? v#) nil (some-> (,form v#) ,@rest)))))))
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p sema -- some_arrow_no_capture`
Expected: PASS

**Step 5: Run the full test suite**

Run: `cargo test`
Expected: All tests pass (no regressions)

**Step 6: Commit**

```bash
git add crates/sema-eval/src/prelude.rs crates/sema/tests/dual_eval_test.rs
git commit -m "fix(prelude): use auto-gensym in some-> to prevent variable capture"
```

---

## Task 6: Edge case tests

**Files:**
- Modify: `crates/sema/tests/dual_eval_test.rs`
- Modify: `crates/sema-reader/src/reader.rs` (reader unit tests)

**Step 1: Add edge case tests**

Add to `crates/sema/tests/dual_eval_test.rs`:

```rust
dual_eval_tests! {
    // Auto-gensym with splicing: macro body can't be captured by user code
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

    // Manual gensym and auto-gensym don't collide (shared counter)
    auto_gensym_no_collision_with_manual: r#"
        (begin
          (define s1 (gensym "x"))
          (defmacro my-m (v) `(let ((x# ,v)) x#))
          (my-m 42))
    "# => Value::int(42),
}
```

Add reader-level edge case tests to `crates/sema-reader/src/reader.rs`:

```rust
#[test]
fn test_auto_gensym_edge_cases() {
    // Multi-# should parse as symbol
    let val = read("x##").unwrap();
    assert_eq!(val.as_symbol().unwrap(), "x##");

    // Keywords are unaffected
    let val = read(":foo").unwrap();
    assert!(val.as_keyword().is_some());
}
```

**Step 2: Run edge case tests**

Run: `cargo test -p sema -- auto_gensym`
Run: `cargo test -p sema-reader -- test_auto_gensym_edge_cases`
Expected: All PASS

**Step 3: Commit**

```bash
git add crates/sema/tests/dual_eval_test.rs crates/sema-reader/src/reader.rs
git commit -m "test: add edge case tests for auto-gensym"
```

---

## Task 7: Update website documentation

**Files:**
- Modify: `website/docs/language/macros-modules.md`

**Step 1: Add auto-gensym documentation**

In `website/docs/language/macros-modules.md`, after the `gensym` section (after line 36), add a new section:

```markdown
### Auto-gensym (`foo#`)

Inside a quasiquote template, any symbol ending with `#` is automatically replaced with a unique generated symbol. All occurrences of the same `foo#` within a single quasiquote resolve to the same gensym, ensuring consistency.

This prevents **variable capture** — a common bug where macro-introduced bindings accidentally shadow user variables.

```sema
;; Without auto-gensym — BUG if user has a variable named "tmp"
(defmacro bad-inc (x)
  `(let ((tmp ,x)) (+ tmp 1)))

(let ((tmp 100))
  (bad-inc tmp))   ; => 2, not 101! "tmp" is captured

;; With auto-gensym — always correct
(defmacro good-inc (x)
  `(let ((tmp# ,x)) (+ tmp# 1)))

(let ((tmp 100))
  (good-inc tmp))  ; => 101 ✓
```

**Rules:**
- Same `foo#` in one quasiquote → same generated symbol
- Each quasiquote evaluation → fresh symbols (no cross-expansion collisions)
- Outside quasiquote, `foo#` is a regular symbol (no magic)
- Works in both the tree-walker and bytecode VM

**Best practice:** Always use auto-gensym for bindings introduced by macros:

```sema
(defmacro swap! (a b)
  `(let ((tmp# ,a))
     (set! ,a ,b)
     (set! ,b tmp#)))
```
```

**Step 2: Update the `gensym` section description**

Update the existing `gensym` section intro to mention auto-gensym as the preferred approach:

```markdown
### `gensym`

Generate a unique symbol manually. For most macro use cases, prefer [auto-gensym (`foo#`)](#auto-gensym-foo) instead.

```sema
(gensym "tmp")   ; => tmp__42 (unique each call)
```
```

**Step 3: Verify website builds**

Run: `cd website && npm run docs:build`
Expected: Build succeeds with no errors

**Step 4: Commit**

```bash
git add website/docs/language/macros-modules.md
git commit -m "docs: document auto-gensym syntax for hygienic macros"
```

---

## Task 8: Full regression test

**Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass

**Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

**Step 3: Run fmt check**

Run: `cargo fmt --check`
Expected: No formatting issues

**Step 4: Manual smoke test**

Run: `cargo run -- -e '(begin (defmacro safe-let (v body) \`(let ((x# ,v)) ,body)) (let ((x 10)) (safe-let 42 x)))'`
Expected: `10` (not `42` — proves the macro's `x#` doesn't capture the user's `x`)

---

## Summary of changes

| File | Change | Lines |
|------|--------|-------|
| `crates/sema-core/src/value.rs` | Shared `next_gensym` function + counter | ~12 |
| `crates/sema-core/src/lib.rs` | Export `next_gensym` | ~1 |
| `crates/sema-stdlib/src/meta.rs` | Use shared `next_gensym`, remove local counter | ~-5 |
| `crates/sema-reader/src/lexer.rs` | Add `#` to `is_symbol_char` | ~1 |
| `crates/sema-reader/src/reader.rs` | Reader unit tests + regression tests | ~30 |
| `crates/sema-eval/src/special_forms.rs` | `is_auto_gensym` + `expand_quasiquote` gensym map | ~25 |
| `crates/sema-vm/src/lower.rs` | Same for VM lowering | ~25 |
| `crates/sema-eval/src/prelude.rs` | `some->`: `__v` → `v#` | ~4 |
| `crates/sema/tests/dual_eval_test.rs` | Dual-eval tests (core + edge cases) | ~80 |
| `website/docs/language/macros-modules.md` | Documentation | ~40 |
| **Total** | | **~210 lines** |
