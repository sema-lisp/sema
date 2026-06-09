# Evaluator Fast-Path Optimization Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Recover the 3.2× performance regression caused by the mini-eval removal, by eliminating per-call overhead in the stdlib→evaluator callback path.

**Architecture:** The stdlib's `call_function` creates a throwaway `EvalContext` on every invocation and routes through `call_callback` (thread-local borrow) → `call_value` → `eval_value` (depth tracking, step counting, trampoline setup). For hot loops like `map`/`filter`/`foldl` over 1M items, this overhead dominates. We fix this by (1) eliminating the throwaway `EvalContext` via a thread-local shared context, (2) adding a `call_value_direct` fast path that skips the full evaluator's per-expression overhead for simple lambda bodies, and (3) removing redundant call-stack/span work from the callback path.

**Tech Stack:** Rust 2021, `sema-core` (thread-local callbacks), `sema-eval` (evaluator), `sema-stdlib` (consumers)

**Benchmark:** `cargo run --release -- benchmarks/1brc/1brc.sema -- benchmarks/1brc/measurements-1m.txt`  
**Baseline (post-regression):** ~3050ms for 1M rows  
**Target:** ≤1500ms (2× improvement, within 1.5× of the old mini-eval's ~960ms)

---

## Diagnosis: Three Sources of Overhead

### Source 1: Throwaway `EvalContext` per call (~allocations)

`call_function` in `list.rs:979-981`:

```rust
pub fn call_function(func: &Value, args: &[Value]) -> Result<Value, SemaError> {
    let ctx = sema_core::EvalContext::new();   // ← allocates 6 RefCells + 3 Cells every call
    sema_core::call_callback(&ctx, func, args)
}
```

`EvalContext::new()` allocates: `RefCell<BTreeMap>`, 3× `RefCell<Vec>`, `RefCell<HashMap>`, plus 3 `Cell<usize>`. For `map` over 1M items, that's 1M × 6 heap allocations. The same problem exists in `io.rs:365,400` for `file/for-each-line` and `file/fold-lines`.

**Fix:** Use a thread-local shared `EvalContext` for all stdlib callbacks.

### Source 2: Thread-local borrow per call (~indirection)

`call_callback` in `context.rs:204-211`:

```rust
pub fn call_callback(ctx: &EvalContext, func: &Value, args: &[Value]) -> Result<Value, SemaError> {
    CALL_FN.with(|call| {
        let borrow = call.borrow();  // ← thread-local access + RefCell borrow per call
        let f = borrow.as_ref().expect("...");
        f(ctx, func, args)
    })
}
```

The thread-local `CALL_FN.with()` + `RefCell::borrow()` is ~5-10ns overhead per call — not the dominant cost, but it adds up over millions of iterations.

**Fix:** Not worth eliminating entirely (the callback architecture is needed), but we can reduce the number of times we go through it by handling `NativeFn` and simple lambdas directly in `call_function`.

### Source 3: Full evaluator overhead per lambda body expression

`call_value` → `eval_value` for each body expression of a lambda:

```rust
pub fn eval_value(ctx: &EvalContext, expr: &Value, env: &Env) -> EvalResult {
    let depth = ctx.eval_depth.get();       // ← Cell::get
    ctx.eval_depth.set(depth + 1);          // ← Cell::set
    if depth == 0 { ctx.eval_steps.set(0); } // ← conditional Cell::set
    if depth > MAX_EVAL_DEPTH { ... }       // ← branch
    let result = eval_value_inner(ctx, expr, env);  // ← trampoline loop setup
    ctx.eval_depth.set(ctx.eval_depth.get().saturating_sub(1));  // ← Cell::get + set
    result
}
```

Then `eval_value_inner` sets up a `CallStackGuard`, reads `eval_step_limit`, creates the trampoline loop. For a lambda body `(+ x 1)`, this is massive overhead relative to the actual work (one symbol lookup + one native call).

**Fix:** `call_value` already handles lambdas directly without going through `eval_value_inner`'s trampoline. But it still calls `eval_value` per body expression, which has the depth/step overhead. We'll optimize `eval_value` itself with a fast path for self-evaluating forms and symbol lookups that skips the overhead.

---

## Tasks

### Task 1: Add thread-local shared `EvalContext` for stdlib callbacks

The biggest single win. Replace `EvalContext::new()` in `call_function` and the IO functions with a thread-local shared context.

**Files:**

- Modify: `crates/sema-core/src/context.rs`
- Modify: `crates/sema-stdlib/src/list.rs`
- Modify: `crates/sema-stdlib/src/io.rs`
- Test: `crates/sema/tests/integration_test.rs` (existing tests)

**Step 1: Add a thread-local `EvalContext` to `sema-core/src/context.rs`**

Add a thread-local shared context and a public accessor function after the existing `CALL_FN` thread-local:

```rust
thread_local! {
    static STDLIB_CTX: EvalContext = EvalContext::new();
}

/// Get a reference to the shared stdlib EvalContext.
/// Use this for stdlib callback invocations instead of creating throwaway contexts.
pub fn with_stdlib_ctx<F, R>(f: F) -> R
where
    F: FnOnce(&EvalContext) -> R,
{
    STDLIB_CTX.with(f)
}
```

Also export `with_stdlib_ctx` from `crates/sema-core/src/lib.rs`.

**Step 2: Update `call_function` in `crates/sema-stdlib/src/list.rs`**

Replace:

```rust
pub fn call_function(func: &Value, args: &[Value]) -> Result<Value, SemaError> {
    let ctx = sema_core::EvalContext::new();
    sema_core::call_callback(&ctx, func, args)
}
```

With:

```rust
pub fn call_function(func: &Value, args: &[Value]) -> Result<Value, SemaError> {
    sema_core::with_stdlib_ctx(|ctx| sema_core::call_callback(ctx, func, args))
}
```

**Step 3: Update `file/for-each-line` and `file/fold-lines` in `crates/sema-stdlib/src/io.rs`**

Replace `let ctx = sema_core::EvalContext::new();` in both functions with `sema_core::with_stdlib_ctx(|ctx| { ... })`. Move the entire loop body inside the `with_stdlib_ctx` closure.

For `file/for-each-line` (around line 365):

```rust
sema_core::with_stdlib_ctx(|ctx| {
    let mut line_buf = String::with_capacity(64);
    loop {
        line_buf.clear();
        let n = reader
            .read_line(&mut line_buf)
            .map_err(|e| SemaError::Io(format!("file/for-each-line {path}: {e}")))?;
        if n == 0 {
            break;
        }
        if line_buf.ends_with('\n') {
            line_buf.pop();
            if line_buf.ends_with('\r') {
                line_buf.pop();
            }
        }
        sema_core::call_callback(ctx, &func, &[Value::string(&line_buf)])?;
    }
    Ok(Value::Nil)
})
```

Same pattern for `file/fold-lines` (around line 400).

**Step 4: Run tests to verify**

Run: `cargo test -p sema --test integration_test 2>&1 | tail -5`
Expected: all tests pass (586 tests)

Also run: `cargo test 2>&1 | tail -5`
Expected: all tests pass

**Step 5: Benchmark**

Run: `cargo build --release && time cargo run --release -- benchmarks/1brc/1brc.sema -- benchmarks/1brc/measurements-1m.txt`
Expected: measurable improvement (likely 10-20% reduction from removing allocations)

**Step 6: Commit**

```bash
git add crates/sema-core/src/context.rs crates/sema-core/src/lib.rs crates/sema-stdlib/src/list.rs crates/sema-stdlib/src/io.rs
git commit -m "perf: use thread-local shared EvalContext for stdlib callbacks

Eliminates per-call allocation of EvalContext in call_function and
file streaming functions. Previously each callback invocation allocated
6 RefCells + 3 Cells; now uses a thread-local shared context."
```

---

### Task 2: Inline `NativeFn` dispatch in `call_function`

Skip the thread-local callback indirection for native functions — they don't need the evaluator at all.

**Files:**

- Modify: `crates/sema-stdlib/src/list.rs`
- Test: existing integration tests

**Step 1: Optimize `call_function` to dispatch `NativeFn` directly**

Replace:

```rust
pub fn call_function(func: &Value, args: &[Value]) -> Result<Value, SemaError> {
    sema_core::with_stdlib_ctx(|ctx| sema_core::call_callback(ctx, func, args))
}
```

With:

```rust
pub fn call_function(func: &Value, args: &[Value]) -> Result<Value, SemaError> {
    match func {
        Value::NativeFn(native) => {
            sema_core::with_stdlib_ctx(|ctx| (native.func)(ctx, args))
        }
        _ => sema_core::with_stdlib_ctx(|ctx| sema_core::call_callback(ctx, func, args)),
    }
}
```

This avoids the `CALL_FN.with()` + `RefCell::borrow()` for the common case of native function callbacks (e.g., when `map` calls `+` or `string/trim`).

**Step 2: Run tests**

Run: `cargo test -p sema --test integration_test 2>&1 | tail -5`
Expected: all pass

**Step 3: Commit**

```bash
git add crates/sema-stdlib/src/list.rs
git commit -m "perf: inline NativeFn dispatch in call_function

Skip thread-local callback indirection when calling native functions
from stdlib higher-order functions like map/filter/fold."
```

---

### Task 3: Add fast path in `eval_value` for self-evaluating forms

The full `eval_value` does depth tracking and step counting even for self-evaluating forms (Int, Float, String, Bool, Nil, Keyword). These don't need any of that — they just return themselves.

**Files:**

- Modify: `crates/sema-eval/src/eval.rs`
- Test: existing integration tests

**Step 1: Add a fast-path check before depth tracking in `eval_value`**

Replace the beginning of `eval_value`:

```rust
pub fn eval_value(ctx: &EvalContext, expr: &Value, env: &Env) -> EvalResult {
    let depth = ctx.eval_depth.get();
    ctx.eval_depth.set(depth + 1);
    // Reset step counter at the top-level eval entry
    if depth == 0 {
        ctx.eval_steps.set(0);
    }
    if depth > MAX_EVAL_DEPTH {
        ctx.eval_depth.set(ctx.eval_depth.get().saturating_sub(1));
        return Err(SemaError::eval(format!(
            "maximum eval depth exceeded ({MAX_EVAL_DEPTH})"
        )));
    }

    let result = eval_value_inner(ctx, expr, env);

    ctx.eval_depth.set(ctx.eval_depth.get().saturating_sub(1));
    result
}
```

With:

```rust
pub fn eval_value(ctx: &EvalContext, expr: &Value, env: &Env) -> EvalResult {
    // Fast path: self-evaluating forms skip depth/step tracking entirely.
    match expr {
        Value::Nil
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Keyword(_)
        | Value::Bytevector(_)
        | Value::NativeFn(_)
        | Value::Lambda(_)
        | Value::HashMap(_) => return Ok(expr.clone()),
        Value::Symbol(spur) => {
            if let Some(val) = env.get(*spur) {
                return Ok(val);
            }
            return Err(SemaError::Unbound(resolve(*spur)));
        }
        _ => {}
    }

    let depth = ctx.eval_depth.get();
    ctx.eval_depth.set(depth + 1);
    if depth == 0 {
        ctx.eval_steps.set(0);
    }
    if depth > MAX_EVAL_DEPTH {
        ctx.eval_depth.set(ctx.eval_depth.get().saturating_sub(1));
        return Err(SemaError::eval(format!(
            "maximum eval depth exceeded ({MAX_EVAL_DEPTH})"
        )));
    }

    let result = eval_value_inner(ctx, expr, env);

    ctx.eval_depth.set(ctx.eval_depth.get().saturating_sub(1));
    result
}
```

This means for a lambda body like `(+ x 1)`, when `eval_value` is called on `x` (Symbol) and `1` (Int), neither hits the depth tracking or trampoline setup. Only the list `(+ x 1)` itself goes through the full path.

**Step 2: Run tests**

Run: `cargo test 2>&1 | tail -5`
Expected: all pass

**Step 3: Benchmark**

Run: `cargo build --release && time cargo run --release -- benchmarks/1brc/1brc.sema -- benchmarks/1brc/measurements-1m.txt`
Expected: significant improvement — self-evaluating forms and symbol lookups are the most common expressions in lambda bodies.

**Step 4: Commit**

```bash
git add crates/sema-eval/src/eval.rs
git commit -m "perf: fast path in eval_value for self-evaluating forms and symbols

Skip depth tracking, step counting, and trampoline setup for Int,
Float, String, Bool, Nil, Keyword, Symbol, and other self-evaluating
values. These are the most common expressions in lambda bodies called
from map/filter/fold hot paths."
```

---

### Task 4: Skip call-stack frame push for callback-invoked lambdas

When `call_value` is called from stdlib (e.g., `map` calling a user lambda), it calls `eval_value` per body expression. Each `eval_value` call enters `eval_value_inner` which sets up a `CallStackGuard`. The `call_value` function itself doesn't push call frames (that's done in `eval_step` for direct calls), but the body expressions can trigger nested frame pushes. The issue is that `eval_value_inner` unconditionally creates a `CallStackGuard` and reads `call_stack_depth()` — a `RefCell::borrow()` on every entry.

**Files:**

- Modify: `crates/sema-eval/src/eval.rs`
- Test: existing integration tests

**Step 1: Optimize `call_value` for simple single-expression lambda bodies**

Most lambdas passed to `map`/`filter`/`fold` have a single body expression. For these, we can avoid the full `eval_value` → `eval_value_inner` path and use `eval_step` directly in a tighter loop.

Add a fast path at the top of the lambda arm in `call_value`:

```rust
Value::Lambda(lambda) => {
    let new_env = Env::with_parent(Rc::new(lambda.env.clone()));

    // Bind parameters (existing code, unchanged)
    if let Some(ref rest) = lambda.rest_param {
        // ... existing rest param binding ...
    } else {
        if args.len() != lambda.params.len() {
            return Err(SemaError::arity(
                lambda.name.as_deref().unwrap_or("lambda"),
                lambda.params.len().to_string(),
                args.len(),
            ));
        }
        for (param, arg) in lambda.params.iter().zip(args.iter()) {
            new_env.set(sema_core::intern(param), arg.clone());
        }
    }

    // Self-reference for recursion
    if let Some(ref name) = lambda.name {
        new_env.set(sema_core::intern(name), Value::Lambda(Rc::clone(lambda)));
    }

    // Evaluate body
    let mut result = Value::Nil;
    for expr in &lambda.body {
        result = eval_value(ctx, expr, &new_env)?;
    }
    Ok(result)
}
```

This part is unchanged. The optimization is in Task 3's fast path — `eval_value` for the body expressions now skips depth tracking for self-evaluating forms and symbols. No further changes needed here.

**Step 2: Instead, reduce `eval_value_inner` overhead for non-TCO calls**

When `eval_value_inner` is entered and the expression is a simple function call (list) that returns a `Trampoline::Value` on the first iteration, the trampoline loop exits immediately. But it still:

1. Clones `expr` into `current_expr` (line 228)
2. Clones `env` into `current_env` (line 229)
3. Calls `ctx.call_stack_depth()` (RefCell borrow, line 230)
4. Creates a `CallStackGuard` (line 231)

We can avoid the clones by taking the expression and env by reference for the first iteration:

Replace the beginning of `eval_value_inner`:

```rust
fn eval_value_inner(ctx: &EvalContext, expr: &Value, env: &Env) -> EvalResult {
    let entry_depth = ctx.call_stack_depth();
    let guard = CallStackGuard { ctx, entry_depth };
    let limit = ctx.eval_step_limit.get();

    // First iteration: use borrowed expr/env to avoid cloning
    if limit > 0 {
        let v = ctx.eval_steps.get() + 1;
        ctx.eval_steps.set(v);
        if v > limit {
            return Err(SemaError::eval("eval step limit exceeded".to_string()));
        }
    }

    match eval_step(ctx, expr, env) {
        Ok(Trampoline::Value(v)) => {
            drop(guard);
            return Ok(v);
        }
        Ok(Trampoline::Eval(next_expr, next_env)) => {
            // Need to continue — enter the trampoline loop
            let mut current_expr = next_expr;
            let mut current_env = next_env;

            // Trim call stack for TCO
            {
                let mut stack = ctx.call_stack.borrow_mut();
                if stack.len() > entry_depth + 1 {
                    let top = stack.last().cloned();
                    stack.truncate(entry_depth);
                    if let Some(frame) = top {
                        stack.push(frame);
                    }
                }
            }

            loop {
                if limit > 0 {
                    let v = ctx.eval_steps.get() + 1;
                    ctx.eval_steps.set(v);
                    if v > limit {
                        return Err(SemaError::eval("eval step limit exceeded".to_string()));
                    }
                }

                match eval_step(ctx, &current_expr, &current_env) {
                    Ok(Trampoline::Value(v)) => {
                        drop(guard);
                        return Ok(v);
                    }
                    Ok(Trampoline::Eval(next_expr, next_env)) => {
                        {
                            let mut stack = ctx.call_stack.borrow_mut();
                            if stack.len() > entry_depth + 1 {
                                let top = stack.last().cloned();
                                stack.truncate(entry_depth);
                                if let Some(frame) = top {
                                    stack.push(frame);
                                }
                            }
                        }
                        current_expr = next_expr;
                        current_env = next_env;
                    }
                    Err(e) => {
                        if e.stack_trace().is_none() {
                            let trace = ctx.capture_stack_trace();
                            drop(guard);
                            return Err(e.with_stack_trace(trace));
                        }
                        drop(guard);
                        return Err(e);
                    }
                }
            }
        }
        Err(e) => {
            if e.stack_trace().is_none() {
                let trace = ctx.capture_stack_trace();
                drop(guard);
                return Err(e.with_stack_trace(trace));
            }
            drop(guard);
            return Err(e);
        }
    }
}
```

This eliminates the unconditional `expr.clone()` and `env.clone()` on entry. For the common case where `eval_step` returns `Trampoline::Value` immediately (non-TCO calls), no cloning happens.

**Step 3: Run tests**

Run: `cargo test 2>&1 | tail -5`
Expected: all pass

**Step 4: Benchmark**

Run: `cargo build --release && time cargo run --release -- benchmarks/1brc/1brc.sema -- benchmarks/1brc/measurements-1m.txt`

**Step 5: Commit**

```bash
git add crates/sema-eval/src/eval.rs
git commit -m "perf: avoid expr/env cloning on first trampoline iteration

Skip the unconditional Value::clone and Env::clone at eval_value_inner
entry. For non-TCO calls (the common case), the first eval_step returns
Trampoline::Value and the clones were wasted."
```

---

### Task 5: Run `make lint` and final benchmark

**Step 1: Lint**

Run: `make lint`
Expected: clean (no warnings, no errors)

**Step 2: Final benchmark comparison**

Run: `cargo build --release && cargo run --release -- benchmarks/1brc/1brc.sema -- benchmarks/1brc/measurements-1m.txt`

Record the result. Compare against:

- Pre-optimization (post-regression): ~3050ms
- Original mini-eval: ~960ms
- Target: ≤1500ms

**Step 3: Update docs/decisions.md**

Add a note to the "Evaluator Callback Architecture" section documenting the fast-path optimizations and the resulting performance numbers.

**Step 4: Commit**

```bash
git add docs/decisions.md
git commit -m "docs: record evaluator fast-path optimization results"
```

---

## Summary of Expected Impact

| Optimization                                       | Mechanism                                              | Expected Impact  |
| -------------------------------------------------- | ------------------------------------------------------ | ---------------- |
| Thread-local shared `EvalContext`                  | Eliminates 6 heap allocs per callback call             | 10-20% reduction |
| Inline `NativeFn` dispatch                         | Skips thread-local + RefCell borrow for native fns     | 5-10% reduction  |
| Self-evaluating fast path in `eval_value`          | Skips depth/step/trampoline for Int, String, Symbol    | 30-50% reduction |
| Avoid first-iteration clones in `eval_value_inner` | Eliminates `Value::clone()` + `Env::clone()` per entry | 10-15% reduction |

Combined target: ≤1500ms on 1BRC 1M rows (vs 3050ms baseline, vs 960ms original mini-eval).
