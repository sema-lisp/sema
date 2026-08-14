# User Context (Ambient Metadata) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a user-facing ambient key-value context to `EvalContext` with scoped overrides, stacks, and integration with logging/LLM calls.

**Architecture:** A `Vec<BTreeMap<Value, Value>>` stack inside `EvalContext` provides scoped push/pop frames. `context/with` pushes a frame, runs a thunk, and pops. Lookups walk the stack top-down (inner shadows outer). A parallel hidden context (`Vec<BTreeMap<Value, Value>>`) stores non-inspectable data. A stacks map (`BTreeMap<Value, Vec<Value>>`) provides Laravel-style append-only lists. All functions are registered in a new `crates/sema-stdlib/src/context.rs` module using `NativeFn::with_ctx` so they receive `&EvalContext`.

**Tech Stack:** Rust, sema-core (`EvalContext`, `Value`, `NativeFn`), sema-stdlib, integration tests in `crates/sema/tests/integration_test.rs`.

---

## Task 1: Add user context storage to `EvalContext`

**Files:**

- Modify: `crates/sema-core/src/context.rs`

**Step 1: Add context fields to `EvalContext`**

Add three new fields after the existing `sandbox` field:

```rust
// In EvalContext struct:
pub user_context: RefCell<Vec<BTreeMap<Value, Value>>>,
pub hidden_context: RefCell<Vec<BTreeMap<Value, Value>>>,
pub context_stacks: RefCell<BTreeMap<Value, Vec<Value>>>,
```

Initialize all three in both `new()` and `new_with_sandbox()`:

```rust
user_context: RefCell::new(vec![BTreeMap::new()]),
hidden_context: RefCell::new(vec![BTreeMap::new()]),
context_stacks: RefCell::new(BTreeMap::new()),
```

Note: start with one empty frame so `context/set` always has a frame to write into.

**Step 2: Add helper methods to `EvalContext`**

Add these methods to the `impl EvalContext` block:

```rust
/// Get a value from user context, walking frames top-down.
pub fn context_get(&self, key: &Value) -> Option<Value> {
    let frames = self.user_context.borrow();
    for frame in frames.iter().rev() {
        if let Some(v) = frame.get(key) {
            return Some(v.clone());
        }
    }
    None
}

/// Set a value in the topmost user context frame.
pub fn context_set(&self, key: Value, value: Value) {
    let mut frames = self.user_context.borrow_mut();
    if let Some(top) = frames.last_mut() {
        top.insert(key, value);
    }
}

/// Check if a key exists in any user context frame.
pub fn context_has(&self, key: &Value) -> bool {
    let frames = self.user_context.borrow();
    for frame in frames.iter().rev() {
        if frame.contains_key(key) {
            return true;
        }
    }
    false
}

/// Remove a key from all user context frames.
pub fn context_remove(&self, key: &Value) -> Option<Value> {
    let mut frames = self.user_context.borrow_mut();
    let mut removed = None;
    for frame in frames.iter_mut().rev() {
        if let Some(v) = frame.remove(key) {
            if removed.is_none() {
                removed = Some(v);
            }
        }
    }
    removed
}

/// Get all user context as a merged map (bottom-up, later frames override).
pub fn context_all(&self) -> BTreeMap<Value, Value> {
    let frames = self.user_context.borrow();
    let mut merged = BTreeMap::new();
    for frame in frames.iter() {
        merged.extend(frame.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    merged
}

/// Push a new empty context frame (for scoped overrides).
pub fn context_push_frame(&self) {
    self.user_context.borrow_mut().push(BTreeMap::new());
}

/// Push a new context frame pre-populated with bindings.
pub fn context_push_frame_with(&self, bindings: BTreeMap<Value, Value>) {
    self.user_context.borrow_mut().push(bindings);
}

/// Pop the topmost context frame.
pub fn context_pop_frame(&self) {
    let mut frames = self.user_context.borrow_mut();
    if frames.len() > 1 {
        frames.pop();
    }
}

// --- Hidden context (same pattern) ---

pub fn hidden_get(&self, key: &Value) -> Option<Value> {
    let frames = self.hidden_context.borrow();
    for frame in frames.iter().rev() {
        if let Some(v) = frame.get(key) {
            return Some(v.clone());
        }
    }
    None
}

pub fn hidden_set(&self, key: Value, value: Value) {
    let mut frames = self.hidden_context.borrow_mut();
    if let Some(top) = frames.last_mut() {
        top.insert(key, value);
    }
}

pub fn hidden_has(&self, key: &Value) -> bool {
    let frames = self.hidden_context.borrow();
    for frame in frames.iter().rev() {
        if frame.contains_key(key) {
            return true;
        }
    }
    false
}

pub fn hidden_push_frame(&self) {
    self.hidden_context.borrow_mut().push(BTreeMap::new());
}

pub fn hidden_pop_frame(&self) {
    let mut frames = self.hidden_context.borrow_mut();
    if frames.len() > 1 {
        frames.pop();
    }
}

// --- Stacks ---

pub fn context_stack_push(&self, key: Value, value: Value) {
    self.context_stacks.borrow_mut()
        .entry(key)
        .or_default()
        .push(value);
}

pub fn context_stack_get(&self, key: &Value) -> Vec<Value> {
    self.context_stacks.borrow()
        .get(key)
        .cloned()
        .unwrap_or_default()
}

pub fn context_stack_pop(&self, key: &Value) -> Option<Value> {
    self.context_stacks.borrow_mut()
        .get_mut(key)
        .and_then(|v| v.pop())
}
```

**Step 3: Add `Value` import if not already present**

The file already imports `Value` via the crate prelude — verify the `use` line includes `Value`. If `BTreeMap` is already imported (it is, for `module_cache`), no additional imports needed.

**Step 4: Run tests to verify no regressions**

Run: `cargo test -p sema-core`
Expected: All existing tests pass. No behavior change — just new fields and methods.

**Step 5: Commit**

```bash
git add crates/sema-core/src/context.rs
git commit -m "feat: add user context, hidden context, and stacks to EvalContext"
```

---

## Task 2: Create `context.rs` stdlib module with core functions

**Files:**

- Create: `crates/sema-stdlib/src/context.rs`
- Modify: `crates/sema-stdlib/src/lib.rs`

**Step 1: Write the failing integration test**

Add to the bottom of `crates/sema/tests/integration_test.rs`:

```rust
#[test]
fn test_context_set_get() {
    assert_eq!(eval("(begin (context/set :name \"alice\") (context/get :name))"), Value::string("alice"));
    assert_eq!(eval("(context/get :missing)"), Value::nil());
}

#[test]
fn test_context_has() {
    assert_eq!(
        eval("(begin (context/set :x 1) (context/has? :x))"),
        Value::bool(true),
    );
    assert_eq!(eval("(context/has? :nope)"), Value::bool(false));
}

#[test]
fn test_context_remove() {
    assert_eq!(
        eval("(begin (context/set :x 1) (context/remove :x) (context/has? :x))"),
        Value::bool(false),
    );
}

#[test]
fn test_context_all() {
    let result = eval("(begin (context/set :a 1) (context/set :b 2) (context/all))");
    // Returns a map {:a 1 :b 2}
    let map = result.as_map_rc().expect("should be a map");
    assert_eq!(map.get(&Value::keyword("a")), Some(&Value::int(1)));
    assert_eq!(map.get(&Value::keyword("b")), Some(&Value::int(2)));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p sema --test integration_test -- test_context_set_get test_context_has test_context_remove test_context_all`
Expected: FAIL — `context/set` not defined.

**Step 3: Create `crates/sema-stdlib/src/context.rs`**

```rust
use sema_core::{EvalContext, NativeFn, SemaError, Value};

use crate::register_fn;

fn register_fn_ctx(
    env: &sema_core::Env,
    name: &str,
    f: impl Fn(&EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::with_ctx(name, f)),
    );
}

pub fn register(env: &sema_core::Env) {
    // (context/set key value)
    register_fn_ctx(env, "context/set", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("context/set", "2", args.len()));
        }
        ctx.context_set(args[0].clone(), args[1].clone());
        Ok(Value::nil())
    });

    // (context/get key) -> value or nil
    register_fn_ctx(env, "context/get", |ctx, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("context/get", "1", args.len()));
        }
        Ok(ctx.context_get(&args[0]).unwrap_or(Value::nil()))
    });

    // (context/has? key) -> bool
    register_fn_ctx(env, "context/has?", |ctx, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("context/has?", "1", args.len()));
        }
        Ok(Value::bool(ctx.context_has(&args[0])))
    });

    // (context/remove key) -> removed value or nil
    register_fn_ctx(env, "context/remove", |ctx, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("context/remove", "1", args.len()));
        }
        Ok(ctx.context_remove(&args[0]).unwrap_or(Value::nil()))
    });

    // (context/all) -> map of all context key-value pairs
    register_fn_ctx(env, "context/all", |ctx, args| {
        if !args.is_empty() {
            return Err(SemaError::arity("context/all", "0", args.len()));
        }
        Ok(Value::map(ctx.context_all()))
    });

    // (context/pull key) -> value, then removes it
    register_fn_ctx(env, "context/pull", |ctx, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("context/pull", "1", args.len()));
        }
        Ok(ctx.context_remove(&args[0]).unwrap_or(Value::nil()))
    });
}
```

**Step 4: Register the module in `lib.rs`**

Add `mod context;` to the module list and `context::register(env);` in `register_stdlib`:

In `crates/sema-stdlib/src/lib.rs`, add after `mod csv_ops;`:

```rust
mod context;
```

In `register_stdlib`, add after `csv_ops::register(env);` (near the bottom):

```rust
context::register(env);
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p sema --test integration_test -- test_context_set_get test_context_has test_context_remove test_context_all`
Expected: All 4 tests PASS.

**Step 6: Commit**

```bash
git add crates/sema-stdlib/src/context.rs crates/sema-stdlib/src/lib.rs crates/sema/tests/integration_test.rs
git commit -m "feat: add context/set, context/get, context/has?, context/remove, context/all, context/pull"
```

---

## Task 3: Add `context/with` scoped override

**Files:**

- Modify: `crates/sema-stdlib/src/context.rs`
- Modify: `crates/sema/tests/integration_test.rs`

**Step 1: Write the failing integration test**

```rust
#[test]
fn test_context_with_scoped() {
    // Inner overrides outer, then restores
    assert_eq!(
        eval(r#"(begin
            (context/set :x "outer")
            (context/with {:x "inner" :y "only-inner"}
                (lambda () (list (context/get :x) (context/get :y)))))"#),
        eval(r#"(list "inner" "only-inner")"#),
    );
    // After context/with, :x is restored and :y is gone
    assert_eq!(
        eval(r#"(begin
            (context/set :x "outer")
            (context/with {:x "inner"} (lambda () nil))
            (list (context/get :x) (context/get :y)))"#),
        eval(r#"(list "outer" nil)"#),
    );
}

#[test]
fn test_context_with_nested() {
    assert_eq!(
        eval(r#"(begin
            (context/set :a 1)
            (context/with {:b 2}
                (lambda ()
                    (context/with {:c 3}
                        (lambda ()
                            (list (context/get :a) (context/get :b) (context/get :c)))))))"#),
        eval("(list 1 2 3)"),
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p sema --test integration_test -- test_context_with_scoped test_context_with_nested`
Expected: FAIL — `context/with` not defined.

**Step 3: Add `context/with` to `context.rs`**

Add this to the `register` function in `context.rs`:

```rust
    // (context/with bindings-map thunk) -> result of thunk
    // Pushes a new context frame with the bindings, calls thunk, then pops.
    register_fn_ctx(env, "context/with", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("context/with", "2", args.len()));
        }
        let bindings = args[0]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
        let thunk = &args[1];
        if thunk.as_lambda_rc().is_none() && thunk.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", thunk.type_name()));
        }

        let frame: std::collections::BTreeMap<Value, Value> = bindings.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        ctx.context_push_frame_with(frame);
        let result = sema_core::call_callback(ctx, thunk, &[]);
        ctx.context_pop_frame();
        result
    });
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema --test integration_test -- test_context_with_scoped test_context_with_nested`
Expected: PASS.

**Step 5: Commit**

```bash
git add crates/sema-stdlib/src/context.rs crates/sema/tests/integration_test.rs
git commit -m "feat: add context/with scoped override"
```

---

## Task 4: Add hidden context functions

**Files:**

- Modify: `crates/sema-stdlib/src/context.rs`
- Modify: `crates/sema/tests/integration_test.rs`

**Step 1: Write the failing integration test**

```rust
#[test]
fn test_context_hidden() {
    // Hidden values are not visible via context/get or context/all
    assert_eq!(
        eval(r#"(begin
            (context/set-hidden :secret "s3cret")
            (list (context/get-hidden :secret) (context/get :secret)))"#),
        eval(r#"(list "s3cret" nil)"#),
    );
}

#[test]
fn test_context_hidden_not_in_all() {
    let result = eval(r#"(begin
        (context/set :visible 1)
        (context/set-hidden :invisible 2)
        (context/all))"#);
    let map = result.as_map_rc().expect("should be map");
    assert_eq!(map.get(&Value::keyword("visible")), Some(&Value::int(1)));
    assert_eq!(map.get(&Value::keyword("invisible")), None);
}

#[test]
fn test_context_has_hidden() {
    assert_eq!(
        eval(r#"(begin (context/set-hidden :k "v") (context/has-hidden? :k))"#),
        Value::bool(true),
    );
    assert_eq!(eval("(context/has-hidden? :nope)"), Value::bool(false));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p sema --test integration_test -- test_context_hidden test_context_hidden_not_in_all test_context_has_hidden`
Expected: FAIL.

**Step 3: Add hidden context functions to `context.rs`**

Add these registrations to the `register` function:

```rust
    // (context/set-hidden key value)
    register_fn_ctx(env, "context/set-hidden", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("context/set-hidden", "2", args.len()));
        }
        ctx.hidden_set(args[0].clone(), args[1].clone());
        Ok(Value::nil())
    });

    // (context/get-hidden key) -> value or nil
    register_fn_ctx(env, "context/get-hidden", |ctx, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("context/get-hidden", "1", args.len()));
        }
        Ok(ctx.hidden_get(&args[0]).unwrap_or(Value::nil()))
    });

    // (context/has-hidden? key) -> bool
    register_fn_ctx(env, "context/has-hidden?", |ctx, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("context/has-hidden?", "1", args.len()));
        }
        Ok(Value::bool(ctx.hidden_has(&args[0])))
    });
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema --test integration_test -- test_context_hidden test_context_hidden_not_in_all test_context_has_hidden`
Expected: PASS.

**Step 5: Commit**

```bash
git add crates/sema-stdlib/src/context.rs crates/sema/tests/integration_test.rs
git commit -m "feat: add context/set-hidden, context/get-hidden, context/has-hidden?"
```

---

## Task 5: Add context stacks (push/pop/get)

**Files:**

- Modify: `crates/sema-stdlib/src/context.rs`
- Modify: `crates/sema/tests/integration_test.rs`

**Step 1: Write the failing integration test**

```rust
#[test]
fn test_context_stack_push_get() {
    assert_eq!(
        eval(r#"(begin
            (context/push :breadcrumbs "first")
            (context/push :breadcrumbs "second")
            (context/push :breadcrumbs "third")
            (context/stack :breadcrumbs))"#),
        eval(r#"(list "first" "second" "third")"#),
    );
}

#[test]
fn test_context_stack_pop() {
    assert_eq!(
        eval(r#"(begin
            (context/push :trail "a")
            (context/push :trail "b")
            (context/pop :trail))"#),
        Value::string("b"),
    );
    // After pop, only "a" remains
    assert_eq!(
        eval(r#"(begin
            (context/push :trail "a")
            (context/push :trail "b")
            (context/pop :trail)
            (context/stack :trail))"#),
        eval(r#"(list "a")"#),
    );
}

#[test]
fn test_context_stack_empty() {
    assert_eq!(eval("(context/stack :empty)"), eval("(list)"));
    assert_eq!(eval("(context/pop :empty)"), Value::nil());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p sema --test integration_test -- test_context_stack_push_get test_context_stack_pop test_context_stack_empty`
Expected: FAIL.

**Step 3: Add stack functions to `context.rs`**

```rust
    // (context/push key value) — append to a named stack
    register_fn_ctx(env, "context/push", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("context/push", "2", args.len()));
        }
        ctx.context_stack_push(args[0].clone(), args[1].clone());
        Ok(Value::nil())
    });

    // (context/stack key) -> list of values in the stack
    register_fn_ctx(env, "context/stack", |ctx, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("context/stack", "1", args.len()));
        }
        Ok(Value::list(ctx.context_stack_get(&args[0])))
    });

    // (context/pop key) -> removes and returns the last value, or nil
    register_fn_ctx(env, "context/pop", |ctx, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("context/pop", "1", args.len()));
        }
        Ok(ctx.context_stack_pop(&args[0]).unwrap_or(Value::nil()))
    });
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema --test integration_test -- test_context_stack_push_get test_context_stack_pop test_context_stack_empty`
Expected: PASS.

**Step 5: Commit**

```bash
git add crates/sema-stdlib/src/context.rs crates/sema/tests/integration_test.rs
git commit -m "feat: add context/push, context/stack, context/pop (stack operations)"
```

---

## Task 6: Add `context/merge` and `context/clear`

**Files:**

- Modify: `crates/sema-stdlib/src/context.rs`
- Modify: `crates/sema-core/src/context.rs`
- Modify: `crates/sema/tests/integration_test.rs`

**Step 1: Write the failing integration test**

```rust
#[test]
fn test_context_merge() {
    assert_eq!(
        eval(r#"(begin
            (context/set :a 1)
            (context/merge {:b 2 :c 3})
            (list (context/get :a) (context/get :b) (context/get :c)))"#),
        eval("(list 1 2 3)"),
    );
}

#[test]
fn test_context_clear() {
    assert_eq!(
        eval(r#"(begin
            (context/set :a 1)
            (context/set :b 2)
            (context/clear)
            (context/all))"#),
        eval("{}"),
    );
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p sema --test integration_test -- test_context_merge test_context_clear`
Expected: FAIL.

**Step 3: Add `context_clear` helper to `EvalContext` in `context.rs` (sema-core)**

```rust
/// Clear all user context frames, resetting to a single empty frame.
pub fn context_clear(&self) {
    let mut frames = self.user_context.borrow_mut();
    frames.clear();
    frames.push(BTreeMap::new());
}
```

**Step 4: Add `context/merge` and `context/clear` to stdlib `context.rs`**

```rust
    // (context/merge map) — merge all key-value pairs from map into current frame
    register_fn_ctx(env, "context/merge", |ctx, args| {
        if args.len() != 1 {
            return Err(SemaError::arity("context/merge", "1", args.len()));
        }
        let map = args[0]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
        for (k, v) in map.iter() {
            ctx.context_set(k.clone(), v.clone());
        }
        Ok(Value::nil())
    });

    // (context/clear) — clear all user context
    register_fn_ctx(env, "context/clear", |ctx, args| {
        if !args.is_empty() {
            return Err(SemaError::arity("context/clear", "0", args.len()));
        }
        ctx.context_clear();
        Ok(Value::nil())
    });
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p sema --test integration_test -- test_context_merge test_context_clear`
Expected: PASS.

**Step 6: Commit**

```bash
git add crates/sema-core/src/context.rs crates/sema-stdlib/src/context.rs crates/sema/tests/integration_test.rs
git commit -m "feat: add context/merge and context/clear"
```

---

## Task 7: Wire context into `log/` functions as automatic metadata

**Files:**

- Modify: `crates/sema-stdlib/src/io.rs` (or wherever `log/info`, `log/warn`, `log/error` are defined)
- Modify: `crates/sema/tests/integration_test.rs`

**Step 1: Find where log functions are registered**

Run: `grep -n "log/info\|log/warn\|log/error" crates/sema-stdlib/src/*.rs`

The log functions likely use `register_fn` (simple). They need to be changed to `register_fn_ctx` so they can read context.

**Step 2: Write the failing integration test**

```rust
#[test]
fn test_log_includes_context() {
    // We can't easily capture stderr in tests, so we test that log functions
    // don't error when context is set — the actual metadata formatting is
    // verified by reading the output format manually.
    // This test ensures the wiring doesn't break.
    eval(r#"(begin
        (context/set :trace-id "abc-123")
        (context/set :user-id 42)
        (log/info "test message"))"#);
}
```

**Step 3: Modify log functions to append context**

Find the log function implementations. Change them from `register_fn` to use `NativeFn::with_ctx`. When context is non-empty, append it after the log message.

Current log format (likely): `[INFO] test message`
New format with context: `[INFO] test message {:trace-id "abc-123" :user-id 42}`

The key change: after formatting the log message, call `ctx.context_all()`. If the resulting map is non-empty, append it as a printed map to the log line.

Example modification pattern:

```rust
// Before (simplified):
register_fn(env, "log/info", |args| {
    eprintln!("[INFO] {}", msg);
    Ok(Value::nil())
});

// After:
env.set(
    sema_core::intern("log/info"),
    Value::native_fn(NativeFn::with_ctx("log/info", |ctx, args| {
        // ... existing message formatting ...
        let context = ctx.context_all();
        if context.is_empty() {
            eprintln!("[INFO] {}", msg);
        } else {
            eprintln!("[INFO] {} {}", msg, Value::map(context));
        }
        Ok(Value::nil())
    })),
);
```

**Step 4: Run tests**

Run: `cargo test -p sema --test integration_test -- test_log_includes_context`
Expected: PASS (no crash; context is silently appended to stderr).

Also run: `cargo test -p sema-stdlib` to verify no regressions.

**Step 5: Commit**

```bash
git add crates/sema-stdlib/src/io.rs crates/sema/tests/integration_test.rs
git commit -m "feat: log/info, log/warn, log/error auto-append user context as metadata"
```

---

## Task 8: Full test suite verification and edge cases

**Files:**

- Modify: `crates/sema/tests/integration_test.rs`

**Step 1: Write edge case tests**

```rust
#[test]
fn test_context_with_restores_on_error() {
    // If the thunk errors, context should still be restored
    let interp = Interpreter::new();
    interp.eval_str(r#"(context/set :x "before")"#).unwrap();
    let _ = interp.eval_str(r#"(context/with {:x "during"} (lambda () (error "boom")))"#);
    assert_eq!(
        interp.eval_str(r#"(context/get :x)"#).unwrap(),
        Value::string("before"),
    );
}

#[test]
fn test_context_with_any_value_types() {
    // Context keys and values can be any Value type
    assert_eq!(
        eval(r#"(begin
            (context/set "string-key" 42)
            (context/set 123 "number-key")
            (list (context/get "string-key") (context/get 123)))"#),
        eval(r#"(list 42 "number-key")"#),
    );
}

#[test]
fn test_context_stacks_independent() {
    // Different stack names are independent
    assert_eq!(
        eval(r#"(begin
            (context/push :a 1)
            (context/push :b 2)
            (list (context/stack :a) (context/stack :b)))"#),
        eval("(list (list 1) (list 2))"),
    );
}

#[test]
fn test_context_pull() {
    assert_eq!(
        eval(r#"(begin
            (context/set :temp "value")
            (define pulled (context/pull :temp))
            (list pulled (context/has? :temp)))"#),
        eval(r#"(list "value" #f)"#),
    );
}
```

**Step 2: Fix `context/with` to restore on error**

In Task 3's implementation of `context/with`, the pop needs to happen even if the thunk errors. Update the implementation:

```rust
    register_fn_ctx(env, "context/with", |ctx, args| {
        if args.len() != 2 {
            return Err(SemaError::arity("context/with", "2", args.len()));
        }
        let bindings = args[0]
            .as_map_rc()
            .ok_or_else(|| SemaError::type_error("map", args[0].type_name()))?;
        let thunk = &args[1];
        if thunk.as_lambda_rc().is_none() && thunk.as_native_fn_rc().is_none() {
            return Err(SemaError::type_error("function", thunk.type_name()));
        }

        let frame: std::collections::BTreeMap<Value, Value> = bindings.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        ctx.context_push_frame_with(frame);
        let result = sema_core::call_callback(ctx, thunk, &[]);
        ctx.context_pop_frame();  // Always pops, even on error
        result
    });
```

Note: this already works because Rust errors (`Result::Err`) don't unwind — `call_callback` returns `Err(...)` normally, and we reach `context_pop_frame()` either way. The code from Task 3 is already correct. But verify with the test.

**Step 3: Run all context tests**

Run: `cargo test -p sema --test integration_test -- test_context`
Expected: All context-related tests PASS.

**Step 4: Run the full test suite**

Run: `cargo test`
Expected: All tests pass, no regressions.

**Step 5: Commit**

```bash
git add crates/sema/tests/integration_test.rs
git commit -m "test: add edge case tests for context (error restore, value types, stacks)"
```

---

## Summary

| Task | What                            | Functions Added                                                                               |
| ---- | ------------------------------- | --------------------------------------------------------------------------------------------- |
| 1    | `EvalContext` storage + helpers | (Rust internals)                                                                              |
| 2    | Core context module             | `context/set`, `context/get`, `context/has?`, `context/remove`, `context/all`, `context/pull` |
| 3    | Scoped overrides                | `context/with`                                                                                |
| 4    | Hidden context                  | `context/set-hidden`, `context/get-hidden`, `context/has-hidden?`                             |
| 5    | Stacks                          | `context/push`, `context/stack`, `context/pop`                                                |
| 6    | Merge + clear                   | `context/merge`, `context/clear`                                                              |
| 7    | Log integration                 | `log/*` auto-appends context as metadata                                                      |
| 8    | Edge cases + verification       | Error restore, type flexibility, full suite                                                   |

Total: **15 new Lisp functions**, all in `context/` namespace.
