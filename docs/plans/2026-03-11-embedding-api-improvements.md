# Embedding API Improvements for Editor Integration

> **Status (2026-07-09):** §4 (coroutines/yielding) is superseded — the cooperative scheduler + `AwaitIo` shipped a different mechanism (see `docs/plans/2026-07-01-cooperative-scheduling.md`). §1–3 (public step-limit control, `register_fn_typed`, `IntoValue`/`FromValue` conversions) remain unbuilt and still relevant for embedders.

> **Context:** Token Editor (a Rust text editor) is planning to embed Sema as its scripting language. During the design phase, several gaps were identified in Sema's public embedding API that create friction or block features. This document captures the needed changes with context and rationale.
>
> **Related:** `/Users/helge/code/token-editor/docs/feature/sema-scripting-integration.md`

**Status:** Planned
**Priority:** P2 (Important -- blocks or significantly impacts Token Editor scripting integration)

---

## Table of Contents

1. [Expose Eval Step Limit on Public API](#1-expose-eval-step-limit-on-public-api)
2. [Typed Function Registration](#2-typed-function-registration)
3. [Automatic Value Conversion Traits](#3-automatic-value-conversion-traits)
4. [Coroutines / Cooperative Yielding](#4-coroutines--cooperative-yielding)

---

## 1. Expose Eval Step Limit on Public API

**Status:** Easy fix -- plumbing already exists internally
**Effort:** S (< 1 hour)
**Blocks:** Token Editor Phase 1 (timeout enforcement)

### Problem

The eval step limit mechanism already exists and works:
- `EvalContext` has `eval_step_limit: Cell<usize>` and `eval_steps: Cell<usize>` (`sema-core/src/context.rs:24-25`)
- The trampoline evaluator checks and increments steps (`sema-eval/src/eval.rs:399-466`)
- It's used by the WASM playground (`sema-wasm/src/lib.rs:1578` -- limit of 10M steps)
- It's used by the fuzzer (`sema-eval/fuzz/fuzz_targets/fuzz_eval.rs:14`)

However, the public `Interpreter` struct in `sema/src/lib.rs` does **not** expose this. An embedder using the public API has no way to set a step limit without reaching into `inner.ctx` (which is private).

### What's Needed

Add two methods to the public API:

```rust
// On InterpreterBuilder -- set limit at construction time
impl InterpreterBuilder {
    /// Set the maximum number of eval steps before execution is terminated.
    /// A limit of 0 means no limit (default).
    pub fn with_step_limit(mut self, limit: usize) -> Self {
        self.step_limit = limit;
        self
    }
}

// On Interpreter -- set/adjust limit at runtime
impl Interpreter {
    /// Set the maximum number of eval steps.
    /// Useful for adjusting limits between script executions.
    /// A limit of 0 means no limit.
    pub fn set_step_limit(&self, limit: usize) {
        self.inner.ctx.set_eval_step_limit(limit);
    }

    /// Reset the step counter to 0.
    /// Call this before each script execution to give it a fresh budget.
    pub fn reset_steps(&self) {
        self.inner.ctx.eval_steps.set(0);
    }
}
```

### Why This Matters

Token Editor needs to enforce a timeout on user scripts (default 100ms, configurable). Without a step limit, a `(while true ...)` script freezes the editor permanently. The alternative -- a watchdog thread that kills the eval via signals or panic -- is fragile and platform-dependent. The step limit mechanism is the correct solution and already works; it just needs to be surfaced.

### Usage in Token Editor

```rust
let interp = InterpreterBuilder::new()
    .with_llm(false)
    .with_step_limit(1_000_000)  // ~100ms on modern hardware
    .build();

// Before each script execution:
interp.reset_steps();
match interp.eval_str(&script_source) {
    Ok(result) => { /* dispatch queued messages */ }
    Err(e) if e.is_step_limit() => {
        show_status_bar_error("Script exceeded execution limit");
    }
    Err(e) => {
        show_status_bar_error(&format!("Script error: {e}"));
    }
}
```

### Additional: Error Type Check

There should also be a way to distinguish step-limit errors from other eval errors. Currently the step limit produces a generic `SemaError::Eval(String)`. Consider adding:

```rust
impl SemaError {
    /// Returns true if this error was caused by exceeding the eval step limit.
    pub fn is_step_limit(&self) -> bool { ... }
}
```

Or add a dedicated variant: `SemaError::StepLimitExceeded { limit: usize, steps: usize }`.

---

## 2. Typed Function Registration

**Status:** New feature
**Effort:** M (2-3 days)
**Blocks:** Nothing (quality-of-life), but significantly reduces Token Editor API module size and bug surface

### Problem

The current `register_fn` signature requires manual type checking for every function:

```rust
interp.register_fn("editor/get-line", |args: &[Value]| {
    // Manual arity check
    if args.len() != 1 {
        return Err(SemaError::arity("editor/get-line", "1", args.len()));
    }
    // Manual type extraction
    let line_num = args[0].as_int()
        .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
    // Manual bounds check
    if line_num < 0 {
        return Err(SemaError::eval("line number must be non-negative"));
    }

    let text = get_line(line_num as usize);
    Ok(Value::string(text))
});
```

Token Editor will register 40+ API functions. Each one needs the same boilerplate: arity check, type extraction, error wrapping. This is tedious, error-prone (easy to forget a check, use wrong index), and obscures the actual logic.

### What's Needed

A typed registration API that auto-extracts arguments and converts return values:

```rust
// Option A: Trait-based auto-extraction
interp.register_fn_typed("editor/get-line", |line_num: i64| -> Result<String> {
    if line_num < 0 {
        return Err(SemaError::eval("line number must be non-negative"));
    }
    Ok(get_line(line_num as usize))
});

// Option B: Macro-based
sema::register! {
    interp, "editor/get-line",
    fn(line_num: i64) -> String {
        get_line(line_num as usize)
    }
}
```

### Design: Conversion Traits

```rust
/// Trait for types that can be extracted from a Sema Value.
pub trait FromValue: Sized {
    fn from_value(value: &Value) -> Result<Self>;
    fn type_name() -> &'static str;
}

/// Trait for types that can be converted into a Sema Value.
pub trait IntoValue {
    fn into_value(self) -> Value;
}
```

Standard implementations:

| Rust Type | Sema Type | `FromValue` | `IntoValue` |
|-----------|-----------|-------------|-------------|
| `i64` | int | `.as_int()` | `Value::int(n)` |
| `f64` | float | `.as_float()` | `Value::float(f)` |
| `bool` | bool | `.as_bool()` | `Value::bool(b)` |
| `String` | string | `.as_str().map(String::from)` | `Value::string(s)` |
| `&str` | string | `.as_str()` | `Value::string(s)` |
| `char` | char | `.as_char()` | `Value::char(c)` |
| `Vec<Value>` | list | `.as_list().map(\|s\| s.to_vec())` | `Value::list(v)` |
| `Option<T>` | T or nil | check nil first | `Value::nil()` or `T::into_value()` |
| `()` | nil | always succeeds | `Value::nil()` |
| `Value` | any | identity | identity |

### Implementation: `register_fn_typed`

Use a trait with implementations for different arities (0 through ~8 args):

```rust
pub trait IntoNativeFn<Args> {
    fn into_native_fn(self, name: &str) -> NativeFn;
}

// For 0-arg functions
impl<F, R> IntoNativeFn<()> for F
where
    F: Fn() -> Result<R> + 'static,
    R: IntoValue,
{
    fn into_native_fn(self, name: &str) -> NativeFn {
        let name = name.to_string();
        NativeFn::simple(&name, move |args: &[Value]| {
            check_arity!(args, &name, 0);
            let result = (self)()?;
            Ok(result.into_value())
        })
    }
}

// For 1-arg functions
impl<F, A, R> IntoNativeFn<(A,)> for F
where
    F: Fn(A) -> Result<R> + 'static,
    A: FromValue,
    R: IntoValue,
{
    fn into_native_fn(self, name: &str) -> NativeFn {
        let name = name.to_string();
        NativeFn::simple(&name, move |args: &[Value]| {
            check_arity!(args, &name, 1);
            let a = A::from_value(&args[0])
                .map_err(|_| SemaError::type_error_at(&name, 0, A::type_name(), args[0].type_name()))?;
            let result = (self)(a)?;
            Ok(result.into_value())
        })
    }
}

// ... up to 8 args

// On Interpreter:
impl Interpreter {
    pub fn register_fn_typed<Args, F>(&self, name: &str, f: F)
    where
        F: IntoNativeFn<Args>,
    {
        let native = f.into_native_fn(name);
        self.inner.global_env.set_str(name, Value::native_fn(native));
    }
}
```

### Usage in Token Editor

Before (current API -- repeated 40+ times):

```rust
interp.register_fn("editor/get-line", |args: &[Value]| {
    if args.len() != 1 {
        return Err(SemaError::arity("editor/get-line", "1", args.len()));
    }
    let n = args[0].as_int()
        .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
    let text = ctx.get_line(n as usize);
    Ok(Value::string(text))
});
```

After (typed API):

```rust
interp.register_fn_typed("editor/get-line", |n: i64| -> Result<String> {
    Ok(ctx.get_line(n as usize))
});
```

The arity check, type extraction, and return value conversion are all handled by the trait machinery. Errors automatically include the function name, argument position, expected type, and actual type.

### Fallback

If this feature isn't prioritized, Token Editor can work around it by defining local helper macros in its own codebase. But having it in Sema benefits all embedders.

---

## 3. Automatic Value Conversion Traits

**Status:** New feature (builds on #2)
**Effort:** M (2-3 days for derive macro, or S for manual trait impls)
**Blocks:** Nothing, but significantly improves ergonomics for returning structured data

### Problem

Many Token Editor API functions return structured data (cursor positions, buffer info, etc.) as Sema maps. Building these maps manually is verbose and error-prone:

```rust
// Current: manual map construction for every structured return
interp.register_fn("editor/get-cursor", |_args: &[Value]| {
    let cursor = ctx.primary_cursor();
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("line"), Value::int(cursor.line as i64));
    map.insert(Value::keyword("column"), Value::int(cursor.column as i64));
    map.insert(Value::keyword("desired-column"),
        cursor.desired_column.map_or(Value::nil(), |c| Value::int(c as i64)));
    Ok(Value::map(map))
});
```

Token Editor has ~10 functions that return maps like this (cursor, selection, buffer info). Each is 5-10 lines of mechanical key-value insertion.

### What's Needed

#### Option A: Derive Macro (Ideal)

A proc macro that auto-generates `IntoValue` and `FromValue` for structs:

```rust
#[derive(IntoSemaValue)]
struct CursorInfo {
    line: i64,
    column: i64,
    #[sema(optional)]
    desired_column: Option<i64>,
}

// Generated: CursorInfo -> Value::map({:line 5 :column 10 :desired-column 3})
// Field names are auto-converted to kebab-case keywords
```

Usage:

```rust
interp.register_fn_typed("editor/get-cursor", || -> Result<CursorInfo> {
    let cursor = ctx.primary_cursor();
    Ok(CursorInfo {
        line: cursor.line as i64,
        column: cursor.column as i64,
        desired_column: cursor.desired_column.map(|c| c as i64),
    })
});
```

#### Option B: Manual Trait Impls (Simpler)

If a proc macro is too much overhead, provide a helper for building maps ergonomically:

```rust
use sema::map_builder;

let cursor_map = map_builder()
    .kw("line", Value::int(cursor.line as i64))
    .kw("column", Value::int(cursor.column as i64))
    .kw_opt("desired-column", cursor.desired_column.map(|c| Value::int(c as i64)))
    .build();
```

This is less magical but still eliminates the `BTreeMap::new()` + repeated `insert()` boilerplate.

### Why This Matters

The Token Editor scripting API will expose structured data in many places:
- `editor/get-cursor` -> `{:line n :column m}`
- `editor/get-cursors` -> list of cursor maps
- `buffer/list` -> list of `{:id n :path "..." :language "..." :modified? bool}`
- Hook event data -> `{:path "..." :language "..." :document-id n}`

Without conversion helpers, each of these is 5-15 lines of `BTreeMap` manipulation. With a derive macro or builder, it's a one-liner return.

### Design Notes

- Field names should convert `snake_case` to `kebab-case` keywords automatically (Lisp convention)
- `Option<T>` fields should become `nil` when `None`
- `Vec<T>` fields should become Sema lists
- Nested structs with `#[derive(IntoSemaValue)]` should recursively convert
- `FromValue` (the reverse direction) is less critical for Token Editor (scripts mostly send simple arguments, not complex maps) but valuable for completeness

### Crate Placement

If using a derive macro: create `sema-derive` crate with the proc macro, re-export from `sema-lang`:

```rust
// sema/src/lib.rs
pub use sema_derive::IntoSemaValue;
```

If using a builder: add `MapBuilder` to `sema-core` or `sema-lang`.

---

## 4. Coroutines / Cooperative Yielding

**Status:** Future / exploratory
**Effort:** XL (significant language feature)
**Blocks:** Token Editor's `editor/prompt` (async callback), interactive script workflows

### Problem

Sema has no mechanism for a script to pause execution, return control to the host, and resume later. This matters for editor scripting patterns like:

```scheme
;; This CANNOT work today:
(token/register-command "rename-symbol"
  (lambda ()
    (let ((old-name (editor/get-selection)))
      ;; This needs to show a text input, wait for user to type,
      ;; then continue with the result. But Sema can't suspend here.
      (let ((new-name (editor/prompt "New name:")))
        (editor/replace-selection new-name)))))
```

The `editor/prompt` call needs to:
1. Pause the script
2. Show a text input in the editor UI
3. Wait for the user to type and press Enter
4. Resume the script with the typed value

This requires either coroutines (cooperative multitasking within the evaluator) or a continuation-passing style that the host can drive.

### How Other Editors Solve This

**Emacs:** `recursive-edit` starts a nested command loop. The calling function is suspended on the C stack. When the user finishes (e.g., presses Enter in the minibuffer), control returns to the suspended function. This works because Emacs owns the entire event loop.

**Neovim:** Lua coroutines + `vim.schedule`. A coroutine yields when it needs async results. The Neovim event loop resumes it when data is available:

```lua
local co = coroutine.create(function()
    local name = coroutine.yield()  -- suspended, waiting for input
    vim.api.nvim_buf_set_text(0, ...)
end)
coroutine.resume(co)  -- start
-- Later, when user types:
coroutine.resume(co, user_input)  -- resume with value
```

**Helix/Steel:** Steel supports async integration with Tokio. Functions can return futures that the Helix event loop drives.

### Possible Approaches for Sema

#### Option A: First-Class Coroutines

Add `coroutine/create`, `coroutine/resume`, `coroutine/yield` to the language:

```scheme
(define co (coroutine/create
  (lambda ()
    (let ((x (coroutine/yield "waiting")))
      (+ x 10)))))

(coroutine/resume co)        ;; => "waiting" (suspended)
(coroutine/resume co 32)     ;; => 42 (completed)
```

Implementation would require saving and restoring the evaluator's continuation (the trampoline stack). This is a significant change to `sema-eval`.

#### Option B: Callback-Based (No Language Change)

The host provides a callback registration mechanism instead of true suspension:

```scheme
;; Instead of synchronous prompt:
(editor/prompt "New name:"
  (lambda (new-name)
    (editor/replace-selection new-name)))
```

This requires no Sema changes -- the host stores the callback `Value` and calls it later via `interp.eval()`. The limitation is that it forces callback-style code (callback hell for sequential operations).

#### Option C: Promise/Future (Middle Ground)

Add a simple promise type that the host can resolve:

```scheme
(define name (editor/prompt "New name:"))  ;; returns a Promise
(then name
  (lambda (new-name)
    (editor/replace-selection new-name)))
```

This is essentially Option B with syntactic sugar and composability (`then`, `all`, `race`).

### Recommendation

**Short term:** Use Option B (callback-based). No Sema changes needed. Token Editor stores the callback and invokes it when the user completes the prompt. Document the callback pattern clearly.

**Medium term:** Consider Option C (promises) as a library-level feature. Can be implemented in Sema without evaluator changes -- promises are just values with registered callbacks.

**Long term:** Option A (coroutines) is the most powerful and ergonomic but requires significant evaluator work. Worth exploring if Sema adoption grows and the embedding use case becomes primary.

### Impact on Token Editor

Without coroutines, Token Editor's `editor/prompt` must use the callback pattern. This is workable for simple cases but becomes unwieldy for scripts that need multiple sequential user interactions. The current plan lists `editor/prompt` as a deferred future feature, so this is not blocking MVP.
