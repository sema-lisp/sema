# Incorporate PR #2 Scheme Features Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Cherry-pick the 5 Scheme features from PR #2 (car/cdr compositions, alist lookup, do loop, char type, delay/force) into the current v0.3.0 codebase which has breaking changes (Spur-based symbols, HashMap variant, memchr).

**Status:** Implemented

**Architecture:** PR #2 was written against v0.2.0 (pre-interning). It cannot be merged directly — there are 5 conflicting files. Instead, we manually port each feature, adapting code to use `Spur`-based symbols/keywords, the new `Value::HashMap` variant in match arms, and the current crate structure.

**Tech Stack:** Rust 2021, lasso (Spur interning), hashbrown, sema workspace crates

---

## Conflict Analysis

**Git merge test result: 5 conflicting files:**

1. `crates/sema-core/src/lib.rs` — PR adds `Thunk` export; main changed to Spur-based exports
2. `crates/sema-core/src/value.rs` — PR adds `Value::Char(char)`, `Value::Thunk(Rc<Thunk>)`, `Thunk` struct; main changed Symbol/Keyword to Spur, added HashMap variant
3. `crates/sema-eval/src/special_forms.rs` — PR adds `do` loop + `delay`/`force` handlers; main changed "begin" | "do" arm and refactored symbol dispatch
4. `crates/sema-stdlib/src/list.rs` — PR adds car/cdr compositions + assq/assv, removes "do" from begin alias; main added Spur-based mini-eval + HashMap support
5. `crates/sema/tests/integration_test.rs` — PR changes `string-ref` to return Char, adds 214 new tests; main changed test structure

**Auto-merged cleanly (no manual work needed):**

- `README.md`, `docs/limitations.md`, `crates/sema-eval/src/eval.rs`, `crates/sema-llm/src/builtins.rs`, `crates/sema-reader/src/reader.rs`, `crates/sema-stdlib/src/map.rs`, `crates/sema-stdlib/src/string.rs`, `website/index.html`

**Files only in PR (new/trivially applicable):**

- `examples/scheme-basics.sema` (new file)
- `crates/sema-reader/src/lexer.rs` (char literal tokenization)
- `crates/sema-stdlib/src/predicates.rs` (char?/promise?/promise-forced?)

---

## Feature-by-Feature Porting Guide

### Feature 1: Character Type (`Value::Char`)

**Effort: Medium** — touches core Value enum, lexer, reader, string.rs, predicates.rs, Display, PartialEq, Ord, Hash

**What PR #2 does:**

- Adds `Value::Char(char)` variant to the Value enum
- Adds `as_char()` method, `Value::char(c)` constructor
- Adds `Char` arms in `PartialEq`, `Ord` (re-numbers type_order), `Display` (formats as `#\a`)
- Lexer: tokenizes `#\a`, `#\space`, `#\newline`, `#\tab`, `#\return`, `#\nul` → `Token::Char(char)`
- Reader: maps `Token::Char` → `Value::Char`
- `string-ref` now returns `Value::Char` instead of `Value::String`
- `string/chars` returns list of Chars instead of Strings
- 15 new char builtins in string.rs: `char->integer`, `integer->char`, char predicates, case conversion, `char->string`, `string->char`, `string->list`, `list->string`
- `char?` predicate in predicates.rs

**Adaptation needed for v0.3.0:**

- The `Value` enum already has `HashMap` variant at position 11 — Char needs to be inserted and type_order adjusted to include both `Char` and `HashMap`
- Hash impl for Value needs `Value::Char(c) => c.hash(state)` arm
- Display for Char: `write!(f, "#\\{}", ...)` with special cases for space/newline/tab

### Feature 2: Lazy Evaluation (`delay`/`force`/`Thunk`)

**Effort: Medium** — new Thunk struct, Value variant, special forms, predicates

**What PR #2 does:**

- Adds `Thunk` struct with `body: Value` and `forced: RefCell<Option<Value>>`
- `Debug` and `Clone` impls for Thunk
- `Value::Thunk(Rc<Thunk>)` variant
- `type_name` returns `"promise"`
- `delay` special form creates lambda wrapping expression, stores in Thunk
- `force` special form evaluates thunk body, memoizes result
- `promise?` and `promise-forced?` predicates

**Adaptation needed for v0.3.0:**

- Thunk struct is independent of interning — ports directly
- Add to Value enum alongside existing HashMap variant
- Add arms in Display, PartialEq, Ord, Hash, type_name, type_order
- `delay`/`force` special forms in special_forms.rs — need to match current dispatch style (string match in `try_eval_special`)
- Export `Thunk` from sema-core lib.rs

### Feature 3: Car/Cdr Compositions (12 functions)

**Effort: Low** — pure stdlib additions in list.rs

**What PR #2 does:**

- Registers caar, cadr, cdar, cddr, caaar, caadr, cadar, caddr, cdaar, cdadr, cddar, cdddr
- Each composes existing `first()` and `rest()` functions

**Adaptation needed for v0.3.0:**

- Ports directly — no dependency on Symbol/Keyword representation
- Just append `register_fn` calls after existing list registrations

### Feature 4: Association Lists (assoc dual-purpose + assq/assv)

**Effort: Low** — modifications to map.rs `assoc` + new functions in list.rs

**What PR #2 does:**

- Makes `assoc` dual-purpose: `(assoc key alist)` for alist lookup, `(assoc map key val ...)` for map assoc
- Adds `assq` and `assv` in list.rs (both use `==` comparison — identical in Sema since we don't have `eq?` vs `eqv?` distinction)

**Adaptation needed for v0.3.0:**

- The `assoc` function in map.rs was modified in v0.3.0 for COW optimization (Rc::try_unwrap) — need to add the alist check before the existing map assoc logic
- `assq`/`assv` port directly

### Feature 5: Proper `do` Loop

**Effort: Medium-High** — replaces `do` as `begin` alias with Scheme iteration form

**What PR #2 does:**

- Removes `"do"` from `"begin" | "do"` match arm in special_forms.rs
- Adds new `eval_do` handler implementing R7RS `do` loop with parallel variable assignment
- `do` special form: `(do ((var init step) ...) (test result ...) body ...)`
- Uses `eval::eval_value` for step evaluation (parallel assignment)

**Adaptation needed for v0.3.0:**

- In current main, special_forms.rs line 24: `"begin" | "do" => Some(eval_begin(args, env))` — remove `| "do"` and add `"do" => Some(eval_do(args, env))`
- The `eval_do` function from PR uses `name.clone()` for Spur binding — must be changed to `intern(name)` pattern
- In list.rs mini-eval, PR removes "do" from `"begin" | "do"` — same change needed, but mini-eval uses Spur comparison now, not string matching. The `sf.do_` Spur constant was for `"do"` matching `begin`. Need to either: (a) remove the `do_` spur and add `do` as its own special form in mini-eval, or (b) simply stop matching `do` to `begin` in the mini-eval (the mini-eval won't handle `do` loops — they'll fall through to the full evaluator)

---

## Task Breakdown

### Task 1: Add `Value::Char` and `Thunk` to sema-core

**Files:**

- Modify: `crates/sema-core/src/value.rs`
- Modify: `crates/sema-core/src/lib.rs`

**Step 1: Add Thunk struct** (after Macro struct, ~line 78)

```rust
/// A lazy promise: delay/force with memoization.
pub struct Thunk {
    pub body: Value,
    pub forced: RefCell<Option<Value>>,
}

impl fmt::Debug for Thunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.forced.borrow().is_some() {
            write!(f, "<promise (forced)>")
        } else {
            write!(f, "<promise>")
        }
    }
}

impl Clone for Thunk {
    fn clone(&self) -> Self {
        Thunk {
            body: self.body.clone(),
            forced: RefCell::new(self.forced.borrow().clone()),
        }
    }
}
```

**Step 2: Add `Char` and `Thunk` variants to Value enum** (Char after Keyword, Thunk after Agent)

```rust
Keyword(Spur),
Char(char),       // NEW
List(Rc<Vec<Value>>),
...
Agent(Rc<Agent>),
Thunk(Rc<Thunk>), // NEW
```

**Step 3: Add arms in `type_name()`**

```rust
Value::Char(_) => "char",
...
Value::Thunk(_) => "promise",
```

**Step 4: Add `as_char()` method and `Value::char()` constructor**

```rust
pub fn as_char(&self) -> Option<char> {
    match self {
        Value::Char(c) => Some(*c),
        _ => None,
    }
}

pub fn char(c: char) -> Value {
    Value::Char(c)
}
```

**Step 5: Add Char/Thunk arms in Hash impl**

```rust
Value::Char(c) => c.hash(state),
```

**Step 6: Add Char arm in PartialEq**

```rust
(Value::Char(a), Value::Char(b)) => a == b,
```

**Step 7: Update type_order and Ord**
Current type*order: Nil=0, Bool=1, Int=2, Float=3, String=4, Symbol=5, Keyword=6, List=7, Vector=8, Map=9, HashMap=10, *=11
New: Nil=0, Bool=1, Int=2, Float=3, Char=4, String=5, Symbol=6, Keyword=7, List=8, Vector=9, Map=10, HashMap=11, \_=12

Add in cmp match:

```rust
(Value::Char(a), Value::Char(b)) => a.cmp(b),
```

**Step 8: Add Display arms**

```rust
Value::Char(c) => match c {
    ' ' => write!(f, "#\\space"),
    '\n' => write!(f, "#\\newline"),
    '\t' => write!(f, "#\\tab"),
    '\r' => write!(f, "#\\return"),
    '\0' => write!(f, "#\\nul"),
    _ => write!(f, "#\\{c}"),
},
...
Value::Thunk(t) => {
    if t.forced.borrow().is_some() {
        write!(f, "<promise (forced)>")
    } else {
        write!(f, "<promise>")
    }
}
```

**Step 9: Update lib.rs exports**
Add `Thunk` to the `pub use value::` line.

**Step 10: Run tests**

```bash
cargo test -p sema-core
```

**Step 11: Commit**

```bash
git add crates/sema-core/
git commit -m "feat(core): add Value::Char and Value::Thunk (delay/force) types"
```

---

### Task 2: Add character literal lexing and parsing

**Files:**

- Modify: `crates/sema-reader/src/lexer.rs`
- Modify: `crates/sema-reader/src/reader.rs`

**Step 1: Add `Token::Char(char)` variant** to the Token enum in lexer.rs.

**Step 2: Add character literal tokenization** in the `'#'` match arm of `tokenize()`, after the `'(' => ...` case for `#(` vectors. Add a `'\\' => { ... }` arm that parses named chars (space, newline, tab, return, nul) and single-char literals.

**Step 3: Add reader case** in `parse_expr()` to map `Token::Char(c) => Ok(Value::Char(*c))`.

**Step 4: Add reader unit tests** for char literals, named chars, special chars, chars in lists, error cases.

**Step 5: Run tests**

```bash
cargo test -p sema-reader
```

**Step 6: Commit**

```bash
git add crates/sema-reader/
git commit -m "feat(reader): add character literal syntax (#\\a, #\\space, etc.)"
```

---

### Task 3: Add car/cdr compositions and alist functions to stdlib

**Files:**

- Modify: `crates/sema-stdlib/src/list.rs`
- Modify: `crates/sema-stdlib/src/map.rs`

**Step 1: Add 12 car/cdr composition functions** at end of `register()` in list.rs, before the closing `}`. Use existing `first()` and `rest()` helper functions.

**Step 2: Add `assq` and `assv`** functions in list.rs.

**Step 3: Make `assoc` dual-purpose** in map.rs — add alist lookup check before existing map assoc logic:

```rust
// Scheme alist lookup: (assoc key alist)
if args.len() == 2 {
    if let Value::List(items) = &args[1] {
        let key = &args[0];
        for pair in items.iter() {
            if let Value::List(p) = pair {
                if !p.is_empty() && &p[0] == key {
                    return Ok(pair.clone());
                }
            }
        }
        return Ok(Value::Bool(false));
    }
}
// existing map assoc logic follows...
```

Also update the error message for insufficient args.

**Step 4: Run tests**

```bash
cargo test -p sema-stdlib
```

**Step 5: Commit**

```bash
git add crates/sema-stdlib/src/list.rs crates/sema-stdlib/src/map.rs
git commit -m "feat(stdlib): add car/cdr compositions and association list functions"
```

---

### Task 4: Add char builtins and predicates

**Files:**

- Modify: `crates/sema-stdlib/src/string.rs`
- Modify: `crates/sema-stdlib/src/predicates.rs`

**Step 1: Change `string-ref` to return `Value::Char`** instead of `Value::String(Rc::new(c.to_string()))`.

**Step 2: Change `string/chars` to return Chars** instead of single-char Strings.

**Step 3: Add 15 char builtins** at end of string.rs register(): `char->integer`, `integer->char`, `char-alphabetic?`, `char-numeric?`, `char-whitespace?`, `char-upper-case?`, `char-lower-case?`, `char-upcase`, `char-downcase`, `char->string`, `string->char`, `string->list`, `list->string`.

**Step 4: Add type predicates** in predicates.rs: `char?`, `promise?`, `promise-forced?`.

**Step 5: Run tests**

```bash
cargo test -p sema-stdlib
```

**Step 6: Commit**

```bash
git add crates/sema-stdlib/src/string.rs crates/sema-stdlib/src/predicates.rs
git commit -m "feat(stdlib): add character builtins and type predicates"
```

---

### Task 5: Add `do` loop and `delay`/`force` special forms

**Files:**

- Modify: `crates/sema-eval/src/special_forms.rs`

**Step 1: Change dispatch** — remove `| "do"` from `"begin" | "do"` match arm. Add:

```rust
"do" => Some(eval_do(args, env)),
"delay" => Some(eval_delay(args, env)),
"force" => Some(eval_force(args, env)),
```

**Step 2: Implement `eval_do`** — R7RS `do` loop with parallel variable assignment:

```rust
fn eval_do(args: &[Value], env: &Env) -> Result<Trampoline, SemaError> {
    // (do ((var init step) ...) (test result ...) body ...)
    if args.len() < 2 {
        return Err(SemaError::eval("do: expected bindings and test clause"));
    }
    // Parse variable bindings
    let bindings = match &args[0] {
        Value::List(l) => l.as_ref(),
        _ => return Err(SemaError::eval("do: expected binding list")),
    };
    let mut var_names = Vec::new();
    let mut steps: Vec<Option<Value>> = Vec::new();
    let loop_env = Env::with_parent(Rc::new(env.clone()));
    for binding in bindings {
        match binding {
            Value::List(parts) if parts.len() >= 2 => {
                let name = match &parts[0] {
                    Value::Symbol(s) => intern(&resolve(*s)),  // adapt for Spur
                    _ => return Err(SemaError::eval("do: variable must be a symbol")),
                };
                let init = eval::eval_value(&parts[1], env)?;
                loop_env.set(name, init);
                var_names.push(name);
                steps.push(if parts.len() >= 3 { Some(parts[2].clone()) } else { None });
            }
            _ => return Err(SemaError::eval("do: invalid binding")),
        }
    }
    // Parse test clause
    let test_clause = match &args[1] {
        Value::List(l) => l.as_ref(),
        _ => return Err(SemaError::eval("do: expected test clause")),
    };
    if test_clause.is_empty() {
        return Err(SemaError::eval("do: test clause cannot be empty"));
    }
    let test_expr = &test_clause[0];
    let result_exprs = &test_clause[1..];
    let body = &args[2..];
    // Main loop
    loop {
        let test_val = eval::eval_value(test_expr, &loop_env)?;
        if test_val.is_truthy() {
            if result_exprs.is_empty() {
                return Ok(Trampoline::Value(Value::Nil));
            }
            for expr in &result_exprs[..result_exprs.len() - 1] {
                eval::eval_value(expr, &loop_env)?;
            }
            return Ok(Trampoline::Eval(result_exprs.last().unwrap().clone(), loop_env));
        }
        for expr in body {
            eval::eval_value(expr, &loop_env)?;
        }
        // Parallel step: evaluate all steps, then assign
        let new_vals: Vec<Option<Value>> = steps.iter()
            .map(|step| step.as_ref().map(|expr| eval::eval_value(expr, &loop_env)).transpose())
            .collect::<Result<_, _>>()?;
        for (name, new_val) in var_names.iter().zip(new_vals.into_iter()) {
            if let Some(val) = new_val {
                loop_env.set(*name, val);
            }
        }
    }
}
```

Note: In the PR version, `var_names` stores `String`. In v0.3.0, we use `Spur` directly since `Env::set` takes `Spur`.

**Step 3: Implement `eval_delay`** — creates a Thunk wrapping a lambda:

```rust
fn eval_delay(args: &[Value], env: &Env) -> Result<Trampoline, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("delay", "1", args.len()));
    }
    use sema_core::Thunk;
    let lambda = Value::Lambda(Rc::new(Lambda {
        params: vec![],
        rest_param: None,
        body: vec![args[0].clone()],
        env: env.clone(),
        name: None,
    }));
    let thunk = Thunk {
        body: lambda,
        forced: std::cell::RefCell::new(None),
    };
    Ok(Trampoline::Value(Value::Thunk(Rc::new(thunk))))
}
```

**Step 4: Implement `eval_force`** — evaluates thunk body, memoizes:

```rust
fn eval_force(args: &[Value], env: &Env) -> Result<Trampoline, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("force", "1", args.len()));
    }
    let val = eval::eval_value(&args[0], env)?;
    match &val {
        Value::Thunk(thunk) => {
            if let Some(cached) = &*thunk.forced.borrow() {
                return Ok(Trampoline::Value(cached.clone()));
            }
            let call_expr = Value::list(vec![thunk.body.clone()]);
            let result = eval::eval_value(&call_expr, env)?;
            *thunk.forced.borrow_mut() = Some(result.clone());
            Ok(Trampoline::Value(result))
        }
        _ => Ok(Trampoline::Value(val)),
    }
}
```

**Step 5: Update mini-eval in list.rs** — remove `sf.do_` from the begin/do check. Currently the SpecialFormSpurs has `do_: Spur` initialized to `"do"`, and the mini-eval matches `head_spur == sf.begin || head_spur == sf.do_` for the begin handler. Remove the `sf.do_` check (and optionally remove the `do_` field from SpecialFormSpurs). The `do` loop is complex enough that it should fall through to the full evaluator.

**Step 6: Run tests**

```bash
cargo test -p sema-eval
```

**Step 7: Commit**

```bash
git add crates/sema-eval/src/special_forms.rs crates/sema-stdlib/src/list.rs
git commit -m "feat(eval): add do loop, delay/force special forms"
```

---

### Task 6: Add integration tests

**Files:**

- Modify: `crates/sema/tests/integration_test.rs`

**Step 1: Add car/cdr composition tests** (test_car_cdr_compositions)
**Step 2: Add alist tests** (test_assoc alist mode, test_assq, test_assv)
**Step 3: Add do loop tests** (basic, factorial, with body, no step, begin still works)
**Step 4: Add char literal tests** (literals, predicate, conversions, char predicates, case, string-ref returns char, string->list, list->string)
**Step 5: Add delay/force tests** (basic, promise?, memoization, force non-promise, promise-forced?)
**Step 6: Update existing tests** that changed behavior:

- `test_string_ref` → expect `Value::Char('h')` not `Value::string("h")`
- `test_string_chars` → expect `(#\a #\b #\c)` not `("a" "b" "c")`
- `test_do_alias` → rename to `test_do_loop`, test as iteration not begin alias

**Step 7: Run all tests**

```bash
cargo test
```

**Step 8: Commit**

```bash
git add crates/sema/tests/integration_test.rs
git commit -m "test: add integration tests for all 5 Scheme features"
```

---

### Task 7: Add examples and update docs

**Files:**

- Create: `examples/scheme-basics.sema`
- Modify: `examples/text-processing.sema` (update caesar cipher to use chars)
- Modify: `docs/limitations.md`
- Modify: `docs/adr.md` (add decision #46+)
- Modify: `CHANGELOG.md`
- Modify: `README.md`
- Modify: `website/index.html`

**Step 1: Create `examples/scheme-basics.sema`** from PR #2.

**Step 2: Update `examples/text-processing.sema`** caesar cipher to use `char-alphabetic?`, `char-upper-case?`, `char->integer`, `integer->char`, `list->string`, `string/chars`.

**Step 3: Update LIMITATIONS.md** — mark the 5 features as implemented, add gap analysis.

**Step 4: Add decisions** to DECISIONS.md (#46: Character type, #47: Lazy evaluation, #48: do loop replaces begin alias).

**Step 5: Update CHANGELOG.md** — add v0.3.1 (or v0.4.0) section.

**Step 6: Update README.md** with new syntax/features.

**Step 7: Update website/index.html** — add char/promise type cards, update stdlib function lists, update code examples.

**Step 8: Run full test suite and example**

```bash
cargo test
cargo run -- examples/scheme-basics.sema
```

**Step 9: Commit**

```bash
git add examples/ agents/ CHANGELOG.md README.md website/
git commit -m "docs: update docs, examples, and changelog for Scheme features"
```

---

## Summary

| Feature              | Effort      | Files Modified | Key Adaptation                                                      |
| -------------------- | ----------- | -------------- | ------------------------------------------------------------------- |
| Char type            | Medium      | 6 files        | Add to Value enum alongside HashMap, update type_order              |
| Thunk/delay/force    | Medium      | 4 files        | Ports cleanly, add to Value enum                                    |
| Car/cdr compositions | Low         | 1 file         | Direct port                                                         |
| Alist functions      | Low         | 2 files        | Add alist check before COW map assoc                                |
| `do` loop            | Medium-High | 2 files        | Adapt var_names from String to Spur, update mini-eval Spur dispatch |

**Total: ~7 tasks, estimated 45-60 min of implementation time.**

The PR's code is well-structured and the features are largely independent of the interning changes. The main friction points are:

1. `Value` enum match exhaustiveness (every match on Value needs new arms)
2. `do` loop using Spur for var names instead of String
3. Mini-eval removing `do` → `begin` alias from Spur-based dispatch
