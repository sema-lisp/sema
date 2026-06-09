# Performance Crate Integration Plan (memchr + lasso + hashbrown)

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Integrate three performance crates (memchr, lasso, hashbrown+ahash) into Sema's interpreter to reduce the 1BRC benchmark from ~1580ms → ~1000ms for 1M rows.

**Status:** Implemented

**Architecture:** Three independent crate integrations, ordered by difficulty: (1) memchr for SIMD byte search in string/split, (2) lasso for string interning of symbols/keywords, (3) hashbrown for fast HashMap in the hot-path accumulator. Each is committed independently and benchmarked.

**Tech Stack:** Rust, `memchr` 2.x, `lasso` 0.7, `hashbrown` 0.15 (includes ahash by default)

**Baseline:** 1580ms for 1M rows (`./target/release/sema examples/1brc.sema -- benchmarks/data/bench-1m.txt`)

---

## Task 1: Add `memchr` — SIMD byte search for `string/split`

**Files:**

- Modify: `Cargo.toml` (workspace deps)
- Modify: `crates/sema-stdlib/Cargo.toml`
- Modify: `crates/sema-stdlib/src/list.rs` (lines 1302–1337, inlined `string/split`)
- Modify: `crates/sema-stdlib/src/string.rs` (lines 71–83, registered `string/split`)
- Test: `crates/sema/tests/integration_test.rs`

### Step 1: Add workspace dependency

In `Cargo.toml` (root), add under `[workspace.dependencies]`:

```toml
memchr = "2"
```

In `crates/sema-stdlib/Cargo.toml`, add:

```toml
memchr.workspace = true
```

### Step 2: Write a test for the behavior being preserved

In `crates/sema/tests/integration_test.rs`, add at the end:

```rust
#[test]
fn test_string_split_memchr() {
    // Basic split
    assert_eq!(
        eval_to_string(r#"(string/split "a;b;c" ";")"#),
        r#"("a" "b" "c")"#
    );
    // Two-part split (1BRC hot path)
    assert_eq!(
        eval_to_string(r#"(string/split "Berlin;12.3" ";")"#),
        r#"("Berlin" "12.3")"#
    );
    // No match
    assert_eq!(
        eval_to_string(r#"(string/split "hello" ";")"#),
        r#"("hello")"#
    );
    // Multi-char separator
    assert_eq!(
        eval_to_string(r#"(string/split "a::b::c" "::")"#),
        r#"("a" "b" "c")"#
    );
    // Empty parts
    assert_eq!(
        eval_to_string(r#"(string/split "a;;b" ";")"#),
        r#"("a" "" "b")"#
    );
}
```

### Step 3: Run test to verify it passes (existing behavior)

Run: `cargo test -p sema --test integration_test -- test_string_split_memchr`
Expected: PASS

### Step 4: Replace manual byte search with memchr in list.rs inlined `string/split`

In `crates/sema-stdlib/src/list.rs`, replace the inlined `string/split` block (lines ~1302–1337). Currently it does:

```rust
if let Some(pos) = bytes.iter().position(|&b| b == sep_byte) {
```

Change to:

```rust
if let Some(pos) = memchr::memchr(sep_byte, bytes) {
```

And replace the second scan:

```rust
if right.as_bytes().iter().any(|&b| b == sep_byte) {
```

With:

```rust
if memchr::memchr(sep_byte, right.as_bytes()).is_some() {
```

Add `use memchr;` at the top of `list.rs` (or use inline path).

### Step 5: Also use memchr in string.rs registered `string/split`

In `crates/sema-stdlib/src/string.rs`, the registered `string/split` (line ~81) currently uses `s.split(sep)`. This is the fallback path. For single-byte separators, add the same memchr optimization:

```rust
register_fn(env, "string/split", |args| {
    if args.len() != 2 {
        return Err(SemaError::arity("string/split", "2", args.len()));
    }
    let s = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let sep = args[1]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
    if sep.len() == 1 {
        let sep_byte = sep.as_bytes()[0];
        let parts: Vec<Value> = s.as_bytes()
            .split(|&b| b == sep_byte)
            .map(|chunk| Value::string(unsafe { std::str::from_utf8_unchecked(chunk) }))
            .collect();
        return Ok(Value::list(parts));
    }
    let parts: Vec<Value> = s.split(sep).map(Value::string).collect();
    Ok(Value::list(parts))
});
```

Actually, simpler and safe: just keep using `s.split(sep)` for the registered version since the hot path uses the inlined version in list.rs. The registered version is only called for non-inlined code paths. No need to complicate this.

### Step 6: Run all tests

Run: `cargo test`
Expected: All PASS

### Step 7: Benchmark

Run: `cargo build --release && ./target/release/sema examples/1brc.sema -- benchmarks/data/bench-1m.txt`
Expected: Small improvement (5-10%, ~1420-1500ms). Record the result.

### Step 8: Commit

```bash
git add -A
git commit -m "perf: use memchr for SIMD byte search in string/split hot path"
```

---

## Task 2: Add `lasso` — String interning for symbols and keywords

This is the largest change. Symbols and keywords will store a `Spur` (u32 key) instead of `Rc<String>`. A thread-local `Rodeo` interner maps between strings and spurs.

**Files:**

- Modify: `Cargo.toml` (workspace deps)
- Modify: `crates/sema-core/Cargo.toml`
- Modify: `crates/sema-core/src/value.rs` (Value enum, Env, constructors, Eq/Ord/Display, accessors)
- Modify: `crates/sema-core/src/lib.rs` (re-export interner)
- Modify: `crates/sema-reader/src/reader.rs` (parse symbols/keywords)
- Modify: `crates/sema-eval/src/eval.rs` (symbol lookup, keyword-as-fn)
- Modify: `crates/sema-eval/src/special_forms.rs` (symbol name matching)
- Modify: `crates/sema-stdlib/src/list.rs` (mini-eval, call_function)
- Modify: `crates/sema-stdlib/src/string.rs` (string<->symbol/keyword conversions)
- Modify: `crates/sema-stdlib/src/map.rs` (if needed)
- Modify: `crates/sema-stdlib/src/io.rs` (if Value::String changed — it's NOT)
- Modify: Various other stdlib files that construct symbols/keywords
- Test: `crates/sema/tests/integration_test.rs`

### Critical design decisions:

- **ONLY** intern `Value::Symbol` and `Value::Keyword`. `Value::String` stays as `Rc<String>`.
- Thread-local `Rodeo` interner, accessed via helper fns `intern(s: &str) -> Spur` and `resolve(spur: Spur) -> &str`.
- `Spur` Ord is by integer value (insertion order), NOT lexicographic. This changes BTreeMap iteration order for maps with keyword/symbol keys. **This is acceptable** — map keys should not depend on iteration order of keywords.
- `Value::symbol("foo")` and `Value::keyword("bar")` constructors will call `intern()`.
- `Value::as_symbol()` and `Value::as_keyword()` will return `Option<&str>` by resolving the spur.
- `Env` keys stay as `String` for now (env lookup is by `&str`, not `Spur`). Env interning is a separate future optimization.

### Step 1: Add workspace dependency

In `Cargo.toml` (root), add under `[workspace.dependencies]`:

```toml
lasso = "0.7"
```

In `crates/sema-core/Cargo.toml`, add:

```toml
lasso.workspace = true
```

### Step 2: Add interner module to sema-core

In `crates/sema-core/src/value.rs`, add a thread-local interner at the top:

```rust
use lasso::{Rodeo, Spur};

thread_local! {
    static INTERNER: RefCell<Rodeo> = RefCell::new(Rodeo::default());
}

/// Intern a string, returning a Spur key.
pub fn intern(s: &str) -> Spur {
    INTERNER.with(|r| r.borrow_mut().get_or_intern(s))
}

/// Resolve a Spur key back to a string.
/// Panics if the spur was not interned (should never happen).
pub fn resolve(spur: Spur) -> String {
    INTERNER.with(|r| r.borrow().resolve(&spur).to_string())
}

/// Resolve a Spur key and call f with the &str.
/// Avoids allocating a String when you just need to read.
pub fn with_resolved<F, R>(spur: Spur, f: F) -> R
where
    F: FnOnce(&str) -> R,
{
    INTERNER.with(|r| {
        let interner = r.borrow();
        f(interner.resolve(&spur))
    })
}
```

### Step 3: Change `Value::Symbol` and `Value::Keyword` variants

In `crates/sema-core/src/value.rs`, change:

```rust
// Before:
Symbol(Rc<String>),
Keyword(Rc<String>),

// After:
Symbol(Spur),
Keyword(Spur),
```

### Step 4: Update constructors

```rust
// Before:
pub fn symbol(s: &str) -> Value {
    Value::Symbol(Rc::new(s.to_string()))
}
pub fn keyword(s: &str) -> Value {
    Value::Keyword(Rc::new(s.to_string()))
}

// After:
pub fn symbol(s: &str) -> Value {
    Value::Symbol(intern(s))
}
pub fn keyword(s: &str) -> Value {
    Value::Keyword(intern(s))
}
```

### Step 5: Update accessors

```rust
// Before:
pub fn as_symbol(&self) -> Option<&str> {
    match self {
        Value::Symbol(s) => Some(s),
        _ => None,
    }
}
pub fn as_keyword(&self) -> Option<&str> {
    match self {
        Value::Keyword(s) => Some(s),
        _ => None,
    }
}

// After:
// These can't return &str directly because the borrow is inside thread_local.
// Return String instead, or use a with_symbol() pattern.
// For maximum compat, return a resolved String wrapper.
// Actually — many call sites do as_symbol().map(|s| s == "foo") or match on the str.
// Best approach: keep returning Option<String> via resolve.
// But this allocates! For hot-path, use with_resolved directly.
// Pragmatic: return Option<String> for now (the alloc is small for identifiers).
// Wait — we can avoid allocation by using a Cow or by restructuring.
// Simplest correct approach: remove as_symbol()/as_keyword(), add as_symbol_spur()/as_keyword_spur()
// and symbol_name()/keyword_name() that return String.
// BUT — changing all call sites is huge.
// BETTER: Use a helper that takes a closure. AND keep as_symbol() returning String for compat.

// Actually, the cleanest approach: provide both:
pub fn as_symbol_spur(&self) -> Option<Spur> {
    match self {
        Value::Symbol(s) => Some(*s),
        _ => None,
    }
}
pub fn as_keyword_spur(&self) -> Option<Spur> {
    match self {
        Value::Keyword(s) => Some(*s),
        _ => None,
    }
}
// Compat accessors (allocate on each call, but only used in non-hot paths):
pub fn as_symbol(&self) -> Option<String> {
    match self {
        Value::Symbol(s) => Some(resolve(*s)),
        _ => None,
    }
}
pub fn as_keyword(&self) -> Option<String> {
    match self {
        Value::Keyword(s) => Some(resolve(*s)),
        _ => None,
    }
}
```

**NOTE:** `as_symbol()` previously returned `Option<&str>`. Changing to `Option<String>` will break call sites that borrow the result. Most usages are like `if let Some(s) = v.as_symbol()` then use `s` — String works fine here. But `&str` slicing won't work. We'll need to fix call sites. The key insight: **most hot path code matches on `Value::Symbol(ref s)` directly** (not through `as_symbol()`), so those just need to switch from `s.as_str()` to `with_resolved(*s, |name| ...)`.

### Step 6: Update PartialEq

```rust
// Symbol and Keyword comparisons are now u32 == u32 (much faster!)
(Value::Symbol(a), Value::Symbol(b)) => a == b,
(Value::Keyword(a), Value::Keyword(b)) => a == b,
```

This is a **free speedup** — same interned string always gets the same Spur.

### Step 7: Update Ord

```rust
// Before:
(Value::Symbol(a), Value::Symbol(b)) => a.cmp(b),
(Value::Keyword(a), Value::Keyword(b)) => a.cmp(b),

// After (integer ordering — NOT lexicographic):
(Value::Symbol(a), Value::Symbol(b)) => {
    // Must compare by string content for deterministic ordering
    with_resolved(*a, |sa| with_resolved(*b, |sb| sa.cmp(sb)))
}
(Value::Keyword(a), Value::Keyword(b)) => {
    with_resolved(*a, |sa| with_resolved(*b, |sb| sa.cmp(sb)))
}
```

**IMPORTANT:** We must compare by string content, not Spur integer value, because:

- The sort order of keyword map keys would change between runs (Spur values depend on intern order)
- `(sort '(:c :a :b))` must always return `(:a :b :c)`, not vary by intern order
- BTreeMap iteration of `{:c 1 :a 2 :b 3}` must be deterministic

The `with_resolved` calls nested like this will call `INTERNER.with` twice, but since it's a thread-local `RefCell`, the inner borrow will panic because the outer borrow is still active.

**Fix:** Use a single borrow:

```rust
(Value::Symbol(a), Value::Symbol(b)) => {
    INTERNER.with(|r| {
        let interner = r.borrow();
        interner.resolve(a).cmp(interner.resolve(b))
    })
}
```

Wait, `INTERNER` is defined in value.rs — this can access it directly. Better yet, add a helper:

```rust
pub fn compare_spurs(a: Spur, b: Spur) -> std::cmp::Ordering {
    if a == b { return std::cmp::Ordering::Equal; }
    INTERNER.with(|r| {
        let interner = r.borrow();
        interner.resolve(&a).cmp(interner.resolve(&b))
    })
}
```

### Step 8: Update Display

```rust
// Before:
Value::Symbol(s) => write!(f, "{s}"),
Value::Keyword(s) => write!(f, ":{s}"),

// After:
Value::Symbol(s) => {
    let name = resolve(*s);
    write!(f, "{name}")
}
Value::Keyword(s) => {
    let name = resolve(*s);
    write!(f, ":{name}")
}
```

### Step 9: Update Lambda struct

The `Lambda` struct stores `params: Vec<String>` and `name: Option<String>`. These stay as `String` — lambda params are used as env keys (which are `String`), and lambda names are displayed. No change needed here.

### Step 10: Update the reader

In `crates/sema-reader/src/reader.rs`:

```rust
// Before:
Ok(Value::Symbol(Rc::new(s.clone())))
// After:
Ok(Value::symbol(s))

// Before:
Ok(Value::Keyword(Rc::new(s.clone())))
// After:
Ok(Value::keyword(s))
```

The `Value::symbol()` and `Value::keyword()` constructors handle interning.

### Step 11: Update eval.rs

In `crates/sema-eval/src/eval.rs`:

```rust
// Line ~261: Symbol lookup
// Before:
Value::Symbol(name) => env
    .get(name)
    .map(Trampoline::Value)
    .ok_or_else(|| SemaError::Unbound(name.to_string())),

// After:
Value::Symbol(spur) => {
    let name = resolve(*spur);
    env.get(&name)
        .map(Trampoline::Value)
        .ok_or_else(|| SemaError::Unbound(name))
}

// Line ~276: Special form dispatch
// Before:
if let Value::Symbol(name) = head {
    if let Some(result) = special_forms::try_eval_special(name, args, env) {

// After:
if let Value::Symbol(spur) = head {
    let resolved = with_resolved(*spur, |name| {
        special_forms::try_eval_special(name, args, env)
    });
    if let Some(result) = resolved {
```

Wait, `try_eval_special` returns `Option<Result<Trampoline, SemaError>>` which borrows nothing from the closure — this works fine.

```rust
// Line ~333: Keyword as function
// Before:
Value::Keyword(kw) => {
    if args.len() != 1 {
        return Err(SemaError::arity(format!(":{kw}"), "1", args.len()));
    }
    ...
    let key = Value::Keyword(Rc::clone(kw));

// After:
Value::Keyword(spur) => {
    if args.len() != 1 {
        let name = resolve(*spur);
        return Err(SemaError::arity(format!(":{name}"), "1", args.len()));
    }
    ...
    let key = Value::Keyword(*spur);
```

### Step 12: Update special_forms.rs

`try_eval_special` takes `head: &str` — this is already resolved by eval.rs before calling, so **no changes needed** in special_forms.rs itself.

Scan for `Value::Symbol(...)` pattern matches in special_forms.rs that extract the inner string:

```rust
// Pattern: if let Value::Symbol(ref name) = args[0] { ... name.to_string() ... }
// Changes to: if let Value::Symbol(spur) = args[0] { ... resolve(spur) ... }
```

This pattern appears many times in special_forms.rs. Each needs updating.

### Step 13: Update list.rs mini-eval

The mini-eval in `crates/sema-stdlib/src/list.rs` has many `Value::Symbol(ref s)` pattern matches:

```rust
// Before:
Value::Symbol(name) => env.get(name).ok_or_else(|| SemaError::Unbound(name.to_string())),

// After:
Value::Symbol(spur) => {
    let name = resolve(*spur);
    env.get(&name).ok_or_else(|| SemaError::Unbound(name))
}
```

```rust
// Before (special form dispatch):
if let Value::Symbol(ref head) = items[0] {
    match head.as_str() {
        "quote" => { ... }

// After:
if let Value::Symbol(head_spur) = items[0] {
    let matched = with_resolved(head_spur, |head_str| -> Option<Result<Value, SemaError>> {
        match head_str {
            "quote" => { ... }
```

Actually this is complex because each arm returns early. Better approach: resolve once, then match:

```rust
if let Value::Symbol(head_spur) = items[0] {
    let head_name = resolve(head_spur);
    match head_name.as_str() {
        "quote" => { ... }
        "if" => { ... }
        ...
    }
}
```

This allocates a String per symbol dispatch, but in the hot path the cost is small compared to what we save on Eq/map-key comparisons. For the hot-path builtins (`assoc`, `get`, `+`, `min`, `max`, etc.), the dispatch overhead is dwarfed by the work they do.

### Step 14: Update string.rs conversions

```rust
// string->symbol: creates Value::Symbol from string
// Before: Ok(Value::symbol(s))  — already correct, uses constructor

// symbol->string: extracts string from symbol
// Before:
let s = args[0].as_symbol().ok_or_else(|| ...)?;
Ok(Value::String(Rc::new(s.to_string())))

// After (as_symbol now returns Option<String>):
let s = args[0].as_symbol().ok_or_else(|| ...)?;
Ok(Value::String(Rc::new(s)))

// keyword->string:
// Before:
Value::Keyword(s) => Ok(Value::String(Rc::new(s.to_string()))),
// After:
Value::Keyword(s) => Ok(Value::String(Rc::new(resolve(*s)))),
```

### Step 15: Update all remaining files that construct/match Symbol or Keyword

Search for `Value::Symbol(Rc::new`, `Value::Keyword(Rc::new`, `Value::Symbol(ref`, `Value::Keyword(ref`, `Rc::clone(kw)` across all crates and update.

Key files to scan:

- `crates/sema-eval/src/special_forms.rs` — many symbol pattern matches
- `crates/sema-stdlib/src/predicates.rs` — `symbol?`, `keyword?`
- `crates/sema-stdlib/src/io.rs` — unlikely
- `crates/sema-llm/src/builtins.rs` — keyword construction for maps
- Any other stdlib file

### Step 16: Re-export interner functions from sema-core lib.rs

In `crates/sema-core/src/lib.rs`, add:

```rust
pub use value::{intern, resolve, with_resolved, compare_spurs};
```

And re-export `Spur` from lasso:

```rust
pub use lasso::Spur;
```

### Step 17: Run all tests

Run: `cargo test`
Expected: All PASS. Fix any compile errors or test failures.

### Step 18: Benchmark

Run: `cargo build --release && ./target/release/sema examples/1brc.sema -- benchmarks/data/bench-1m.txt`
Expected: 20-40% improvement (~950-1260ms). Record the result.

### Step 19: Commit

```bash
git add -A
git commit -m "perf: intern symbols and keywords with lasso (Spur-based Value::Symbol/Keyword)"
```

### Step 20: Add decision to DECISIONS.md

Add Decision #43 documenting the interning approach:

```markdown
### 43. String interning for symbols and keywords (lasso)

- `Value::Symbol` and `Value::Keyword` store `Spur` (u32) instead of `Rc<String>`
- Thread-local `Rodeo` interner, accessed via `intern()/resolve()/with_resolved()`
- `Value::String` remains `Rc<String>` — arbitrary user strings are NOT interned
- Eq comparison of symbols/keywords is now O(1) integer comparison
- Ord comparison still resolves to lexicographic for deterministic BTreeMap ordering
- Consistent with existing `thread_local!` pattern (LLM provider, module cache)
```

---

## Task 3: Add `hashbrown` — Fast HashMap for hot-path accumulator

**Files:**

- Modify: `Cargo.toml` (workspace deps)
- Modify: `crates/sema-core/Cargo.toml`
- Modify: `crates/sema-core/src/value.rs` (add Hash impl for Value)
- Modify: `crates/sema-stdlib/Cargo.toml`
- Modify: `crates/sema-stdlib/src/list.rs` (inlined `assoc`/`get` for HashMap fast path)
- Test: `crates/sema/tests/integration_test.rs`

### Design decisions:

- **Do NOT change `Value::Map` from BTreeMap to HashMap globally.** This would break deterministic iteration order (sorted output, test stability).
- Instead, add `Value::HashMap(Rc<hashbrown::HashMap<Value, Value>>)` as a new variant, OR use HashMap only inside the inlined `assoc`/`get` hot path.
- Actually, adding a new Value variant is invasive (every match on Value needs updating). Better approach: **Add `Hash` impl for `Value`** so it CAN be used as HashMap key, then use hashbrown HashMap internally in a new native function `hashmap/new`, `hashmap/get`, `hashmap/assoc` — or even simpler: make `file/fold-lines` auto-detect when the accumulator is a map with many keys and switch to HashMap internally.
- **Simplest high-impact approach:** Add a `Hash` impl for `Value`, then create a specialized native fold function that uses `hashbrown::HashMap` internally and converts back to `BTreeMap` at the end. This is the most contained change.
- **Even simpler:** Just add `Hash` to `Value` and introduce a `hashmap` Sema type that wraps `hashbrown::HashMap`. Provide `hashmap/new`, `hashmap/get`, `hashmap/assoc`, `hashmap/to-map` builtins. Let users choose.

After reflection, the **cleanest approach with maximum impact** is:

**Add `Value::HashMap` variant** + thin API layer. The 1BRC script would use `(hashmap)` instead of `{}` for the accumulator, and `hashmap/assoc` / `hashmap/get` instead of `assoc` / `get`. Then convert to sorted map for output. This avoids modifying the existing `Value::Map` path.

BUT — this means changing the 1BRC script and adding a bunch of match arms. A better approach for the hot path:

**Optimize the inlined `assoc` and `get` in list.rs to use hashbrown HashMap when the map exceeds a threshold (e.g., 32 entries).** Store the HashMap alongside the BTreeMap inside the existing `Value::Map` as a cache. Too complex.

**Final approach (pragmatic):** Add `Hash` impl for `Value`. In the inlined `assoc`/`get` in list.rs, when the map has > 16 entries and we're doing a `get` with a known key type (String for station names in 1BRC), use a hashbrown-backed lookup. This requires keeping a shadow HashMap that's built on first use.

**Actually, the best pragmatic approach:** Just make `get` faster by adding Hash to Value and providing an alternative function. BUT looking at the 1BRC code more carefully — the accumulator is a `Value::Map(Rc<BTreeMap<Value, Value>>)`. Each row does `(get acc name)` and `(assoc acc name ...)`. With interning done (Task 2), keyword keys like `:min`, `:max` are now `Spur` (u32). But station names are `Value::String` — NOT interned. So the BTreeMap lookup for station names is O(log 400) string comparisons.

**Revised approach:** Add `Hash` for `Value` and provide opt-in HashMap-based map operations as a new `Value::HashMap` variant. This gives the 1BRC script a 10-25% boost on the accumulator lookups.

### Step 1: Add workspace dependency

In `Cargo.toml` (root), add under `[workspace.dependencies]`:

```toml
hashbrown = "0.15"
```

In `crates/sema-core/Cargo.toml` and `crates/sema-stdlib/Cargo.toml`, add:

```toml
hashbrown.workspace = true
```

### Step 2: Add Hash impl for Value

In `crates/sema-core/src/value.rs`, add:

```rust
use std::hash::{Hash, Hasher};

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Value::Nil => {}
            Value::Bool(b) => b.hash(state),
            Value::Int(n) => n.hash(state),
            Value::Float(f) => f.to_bits().hash(state),
            Value::String(s) => s.hash(state),
            Value::Symbol(s) => s.hash(state),
            Value::Keyword(s) => s.hash(state),
            Value::List(l) => l.hash(state),
            Value::Vector(v) => v.hash(state),
            // Maps, functions, etc. hash by identity/pointer
            _ => {}
        }
    }
}
```

### Step 3: Add `Value::HashMap` variant

In `crates/sema-core/src/value.rs`:

```rust
HashMap(Rc<hashbrown::HashMap<Value, Value>>),
```

Add the variant to `type_name()`, `Display`, `PartialEq`, `Ord`, and all other match blocks. Type name: `"hashmap"`. Display: same as Map (`{k v ...}`).

For Eq: `HashMap` compares equal to another `HashMap` with same entries. For Ord: compare by converting to sorted entries.

### Step 4: Write tests

```rust
#[test]
fn test_hashmap_basic() {
    assert_eq!(eval_to_string("(hashmap/get (hashmap/new :a 1 :b 2) :a)"), "1");
    assert_eq!(eval_to_string("(hashmap/get (hashmap/new :a 1 :b 2) :c)"), "nil");
    assert_eq!(eval_to_string("(hashmap/get (hashmap/new :a 1) :a 99)"), "1");
    assert_eq!(eval_to_string("(hashmap/get (hashmap/new) :a 99)"), "99");
}

#[test]
fn test_hashmap_assoc() {
    assert_eq!(
        eval_to_string("(hashmap/get (hashmap/assoc (hashmap/new) :a 1) :a)"),
        "1"
    );
}

#[test]
fn test_hashmap_to_map() {
    // Convert hashmap to sorted BTreeMap for deterministic output
    assert_eq!(
        eval_to_string("(hashmap/to-map (hashmap/new :b 2 :a 1))"),
        "{:a 1 :b 2}"
    );
}

#[test]
fn test_hashmap_keys_vals() {
    // keys/vals should work on hashmaps (order may vary, so test via sort)
    assert_eq!(
        eval_to_string("(sort (hashmap/keys (hashmap/new :b 2 :a 1)))"),
        "(:a :b)"
    );
}
```

### Step 5: Register hashmap builtins

In `crates/sema-stdlib/src/map.rs`, add hashmap builtins:

```rust
use hashbrown::HashMap as HBHashMap;

register_fn(env, "hashmap/new", |args| {
    if args.len() % 2 != 0 {
        return Err(SemaError::eval("hashmap/new: requires even number of arguments"));
    }
    let mut map = HBHashMap::with_capacity(args.len() / 2);
    for pair in args.chunks(2) {
        map.insert(pair[0].clone(), pair[1].clone());
    }
    Ok(Value::HashMap(Rc::new(map)))
});

register_fn(env, "hashmap/get", |args| {
    if args.len() < 2 || args.len() > 3 {
        return Err(SemaError::arity("hashmap/get", "2-3", args.len()));
    }
    let default = if args.len() == 3 { args[2].clone() } else { Value::Nil };
    match &args[0] {
        Value::HashMap(map) => Ok(map.get(&args[1]).cloned().unwrap_or(default)),
        Value::Map(map) => Ok(map.get(&args[1]).cloned().unwrap_or(default)),
        _ => Err(SemaError::type_error("hashmap or map", args[0].type_name())),
    }
});

register_fn(env, "hashmap/assoc", |args| {
    if args.len() < 3 || args.len() % 2 != 1 {
        return Err(SemaError::eval("hashmap/assoc: requires hashmap and even number of key-value pairs"));
    }
    let mut map = match args[0].clone() {
        Value::HashMap(m) => match Rc::try_unwrap(m) {
            Ok(map) => map,
            Err(m) => m.as_ref().clone(),
        },
        Value::Map(m) => {
            // Convert BTreeMap to HashMap
            let mut hm = HBHashMap::with_capacity(m.len() + args.len() / 2);
            for (k, v) in m.iter() {
                hm.insert(k.clone(), v.clone());
            }
            hm
        }
        _ => return Err(SemaError::type_error("hashmap or map", args[0].type_name())),
    };
    for pair in args[1..].chunks(2) {
        map.insert(pair[0].clone(), pair[1].clone());
    }
    Ok(Value::HashMap(Rc::new(map)))
});

register_fn(env, "hashmap/to-map", |args| {
    if args.len() != 1 {
        return Err(SemaError::arity("hashmap/to-map", "1", args.len()));
    }
    match &args[0] {
        Value::HashMap(hm) => {
            let btree: BTreeMap<Value, Value> = hm.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            Ok(Value::Map(Rc::new(btree)))
        }
        Value::Map(_) => Ok(args[0].clone()), // Already a sorted map
        _ => Err(SemaError::type_error("hashmap or map", args[0].type_name())),
    }
});

register_fn(env, "hashmap/keys", |args| {
    if args.len() != 1 {
        return Err(SemaError::arity("hashmap/keys", "1", args.len()));
    }
    match &args[0] {
        Value::HashMap(hm) => Ok(Value::list(hm.keys().cloned().collect())),
        _ => Err(SemaError::type_error("hashmap", args[0].type_name())),
    }
});

register_fn(env, "hashmap/contains?", |args| {
    if args.len() != 2 {
        return Err(SemaError::arity("hashmap/contains?", "2", args.len()));
    }
    match &args[0] {
        Value::HashMap(hm) => Ok(Value::Bool(hm.contains_key(&args[1]))),
        _ => Err(SemaError::type_error("hashmap", args[0].type_name())),
    }
});
```

### Step 6: Make `get`, `assoc`, `keys`, `vals`, `contains?`, `length`, `count`, `empty?` work with HashMap too

Update the existing registered builtins to handle `Value::HashMap` in addition to `Value::Map`. This way user code that calls `(get hm :key)` works without needing `hashmap/get`.

In `crates/sema-stdlib/src/map.rs`, update `get`:

```rust
Value::HashMap(map) => Ok(map.get(&args[1]).cloned().unwrap_or(default)),
```

Similarly update `assoc`, `keys`, `vals`, `contains?`, `count`, `empty?`, `length`, `merge`.

### Step 7: Update inlined `get` and `assoc` in list.rs mini-eval

The inlined `get` (line ~1173) and `assoc` (line ~1116) should handle `Value::HashMap`:

For inlined `get`:

```rust
"get" => {
    if items.len() == 3 || items.len() == 4 {
        let map_val = sema_eval_value(&items[1], env)?;
        match &map_val {
            Value::Map(m) => {
                let key = sema_eval_value(&items[2], env)?;
                let default = if items.len() == 4 { sema_eval_value(&items[3], env)? } else { Value::Nil };
                return Ok(m.get(&key).cloned().unwrap_or(default));
            }
            Value::HashMap(m) => {
                let key = sema_eval_value(&items[2], env)?;
                let default = if items.len() == 4 { sema_eval_value(&items[3], env)? } else { Value::Nil };
                return Ok(m.get(&key).cloned().unwrap_or(default));
            }
            _ => return Err(SemaError::type_error("map", map_val.type_name())),
        }
    }
}
```

For inlined `assoc` with COW optimization — handle `Value::HashMap` with `Rc::make_mut`:

```rust
// In the assoc inlined builtin, after taking from env:
// If the value is a HashMap, use Rc::make_mut on it too
Some(Value::HashMap(m)) => {
    // HashMap COW path
    let mut map_rc = m;
    let map = Rc::make_mut(&mut map_rc);
    for pair in items[2..].chunks(2) {
        let key = sema_eval_value(&pair[0], env)?;
        let val = sema_eval_value(&pair[1], env)?;
        map.insert(key, val);
    }
    return Ok(Value::HashMap(map_rc));
}
```

### Step 8: Update the 1BRC script

Change `examples/1brc.sema` to use `hashmap/new` for the accumulator and `hashmap/to-map` for sorted output:

```lisp
;; Change initial accumulator from {} to (hashmap/new)
(define result
  (file/fold-lines input-file
    (fn (acc line)
      (if (= line "")
          acc
          (let ((parts (string/split line ";")))
            (let ((name (first parts))
                  (temp (float (string->number (nth parts 1)))))
              (let ((existing (get acc name)))
                (if (nil? existing)
                    (assoc acc name {:min temp :max temp :sum temp :count 1})
                    (assoc acc name
                      {:min   (min temp (get existing :min))
                       :max   (max temp (get existing :max))
                       :sum   (+ temp (get existing :sum))
                       :count (+ 1 (get existing :count))})))))))
    (hashmap/new)))

;; Convert to sorted map for output
(define sorted-result (hashmap/to-map result))
```

Wait — the sub-maps (stats with `:min`, `:max`, etc.) are still `{}` BTreeMaps. That's fine — they have only 4 keys, BTreeMap is competitive there. The outer accumulator with ~400 station entries is what benefits from HashMap.

Also, `assoc` on a HashMap should return a HashMap (handled by the inlined assoc). And `get` on a HashMap should work (handled by inlined get).

The `keys` and `length` calls on `result` need to work with HashMap too.

### Step 9: Run all tests

Run: `cargo test`
Expected: All PASS.

### Step 10: Benchmark

Run: `cargo build --release && ./target/release/sema examples/1brc.sema -- benchmarks/data/bench-1m.txt`
Expected: Additional 10-25% improvement over Task 2. Record the result.

### Step 11: Commit

```bash
git add -A
git commit -m "perf: add hashbrown HashMap variant for fast O(1) accumulator lookups"
```

### Step 12: Add decision to DECISIONS.md

```markdown
### 44. HashMap variant for performance-critical accumulation (hashbrown)

- Added `Value::HashMap(Rc<hashbrown::HashMap<Value, Value>>)` as opt-in fast map
- `hashmap/new`, `hashmap/get`, `hashmap/assoc`, `hashmap/to-map`, `hashmap/keys`, `hashmap/contains?` builtins
- Existing `get`, `assoc`, `keys`, `vals`, `contains?`, `count`, `empty?` also work on HashMap
- `Value::Map` (BTreeMap) remains the default for deterministic ordered output
- HashMap used where O(1) lookup matters more than key ordering (e.g., 1BRC accumulator with ~400 entries)
- `Hash` impl added for `Value`: hashes discriminant + inner value; functions/maps hash by discriminant only
- COW optimization (Rc::make_mut) applies to HashMap assoc just like BTreeMap assoc
```

---

## Verification Checklist

After all 3 tasks:

1. `cargo test` — all tests pass
2. `cargo build --release && ./target/release/sema examples/1brc.sema -- benchmarks/data/bench-1m.txt` — target ~1000ms
3. `./target/release/sema examples/1brc.sema -- benchmarks/data/bench-10m.txt` — verify correctness on larger dataset
4. Run the REPL (`cargo run --release`) and test interactively:
   - `(define m {:a 1 :b 2})` — BTreeMap still works
   - `(:a m)` — keyword-as-function still works
   - `(get m :a)` — get still works
   - `(string/split "a;b;c" ";")` — split still works
   - `(define hm (hashmap/new :x 1 :y 2))` — hashmap works
   - `(get hm :x)` — get works on hashmap
   - `(hashmap/to-map hm)` — conversion works
5. No regressions in existing examples: `cargo run --release -- examples/hello.sema`

## Summary

| Task | Crate     | Expected Improvement | Effort                          |
| ---- | --------- | -------------------- | ------------------------------- |
| 1    | memchr    | 5-10%                | Small (< 1 hour)                |
| 2    | lasso     | 20-40%               | Large (many files)              |
| 3    | hashbrown | 10-25%               | Medium (new variant + builtins) |

**Combined target: ~1000ms** (from 1580ms baseline, ~37% reduction)
