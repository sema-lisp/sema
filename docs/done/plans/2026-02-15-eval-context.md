# EvalContext: Replace Thread-Locals with Explicit Context

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace all `thread_local!` state in `sema-eval` with an explicit `EvalContext` struct threaded through the evaluator, enabling multiple independent interpreter instances per thread.

**Status:** Implemented

**Architecture:** Define `EvalContext` in `sema-core` so all crates can reference it. Use a dual-constructor strategy for `NativeFn` so the ~330 builtins that don't need context remain unchanged, while the ~20 that do get context access. Thread `&EvalContext` through the eval loop, special forms, and into native function calls. LLM state stays in `sema-llm` but captures `Rc<RefCell<...>>` in closures instead of using thread-locals.

**Tech Stack:** Rust 2021, `Rc<RefCell<...>>` for interior mutability (single-threaded), existing crate layering preserved.

---

## Key Design Decisions

### Scope of each current thread-local

| Variable                | Current scope | New scope                                    | Rationale                                        |
| ----------------------- | ------------- | -------------------------------------------- | ------------------------------------------------ |
| `MODULE_CACHE`          | Per-thread    | Per-interpreter                              | Each interpreter should have independent modules |
| `CURRENT_FILE`          | Per-thread    | Per-context (stack)                          | Scoped to eval invocation chain                  |
| `MODULE_EXPORTS`        | Per-thread    | Per-context (stack of `Option<Vec<String>>`) | Already a stack for nested import reentrancy     |
| `MODULE_LOAD_STACK`     | Per-thread    | Per-context (stack)                          | Cyclic import detection                          |
| `CALL_STACK`            | Per-thread    | Per-context                                  | Scoped to eval invocation chain                  |
| `SPAN_TABLE`            | Per-thread    | Per-interpreter                              | Spans persist across eval calls (with 200K cap)  |
| `EVAL_DEPTH`            | Per-thread    | Per-context                                  | Tracks nesting depth                             |
| `EVAL_STEP_LIMIT`       | Per-thread    | Per-context                                  | Set once, read per eval                          |
| `EVAL_STEPS`            | Per-thread    | Per-context                                  | Reset at top-level eval                          |
| `SF` (special_forms.rs) | Per-thread    | **Keep as thread-local**                     | Pure cache of interned symbols, no state         |

### NativeFn strategy: dual constructors, not 350 signature changes

The stored closure type changes to accept `&EvalContext`, but a wrapper constructor hides it from builtins that don't need it:

```rust
// In sema-core:
pub type NativeFnInner = dyn Fn(&EvalContext, &[Value]) -> Result<Value, SemaError>;

impl NativeFn {
    // For the ~330 builtins that don't need context:
    pub fn simple(name: impl Into<String>, f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static) -> Self { ... }
    // For the ~20 that do:
    pub fn with_ctx(name: impl Into<String>, f: impl Fn(&EvalContext, &[Value]) -> Result<Value, SemaError> + 'static) -> Self { ... }
}
```

### Interior mutability: `RefCell`/`Cell` inside EvalContext

The eval loop needs to hold `&EvalContext` while also mutating the call stack (via RAII guards). Using `RefCell` inside the struct avoids borrow-checker fights with `&mut`:

```rust
pub struct EvalContext {
    pub module_cache: RefCell<BTreeMap<PathBuf, BTreeMap<String, Value>>>,
    pub current_file: RefCell<Vec<PathBuf>>,
    pub module_exports: RefCell<Vec<Option<Vec<String>>>>,
    pub module_load_stack: RefCell<Vec<PathBuf>>,
    pub call_stack: RefCell<Vec<CallFrame>>,
    pub span_table: RefCell<HashMap<usize, Span>>,
    pub eval_depth: Cell<usize>,
    pub eval_step_limit: Cell<usize>,
    pub eval_steps: Cell<usize>,
}
```

---

## Phase 1: Define EvalContext in sema-core

### Task 1: Add EvalContext struct to sema-core

**Files:**

- Create: `crates/sema-core/src/context.rs`
- Modify: `crates/sema-core/src/lib.rs`

**Step 1: Create the context module**

Create `crates/sema-core/src/context.rs`:

```rust
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use crate::{CallFrame, SemaError, Span, SpanMap, StackTrace, Value};

const MAX_SPAN_TABLE_ENTRIES: usize = 200_000;

/// Evaluation context — holds all mutable interpreter state.
///
/// Uses interior mutability (`RefCell`/`Cell`) so the eval loop
/// can hold `&EvalContext` while RAII guards mutate the call stack.
pub struct EvalContext {
    pub module_cache: RefCell<BTreeMap<PathBuf, BTreeMap<String, Value>>>,
    pub current_file: RefCell<Vec<PathBuf>>,
    pub module_exports: RefCell<Vec<Option<Vec<String>>>>,
    pub module_load_stack: RefCell<Vec<PathBuf>>,
    pub call_stack: RefCell<Vec<CallFrame>>,
    pub span_table: RefCell<HashMap<usize, Span>>,
    pub eval_depth: Cell<usize>,
    pub eval_step_limit: Cell<usize>,
    pub eval_steps: Cell<usize>,
}

impl EvalContext {
    pub fn new() -> Self {
        Self {
            module_cache: RefCell::new(BTreeMap::new()),
            current_file: RefCell::new(Vec::new()),
            module_exports: RefCell::new(Vec::new()),
            module_load_stack: RefCell::new(Vec::new()),
            call_stack: RefCell::new(Vec::new()),
            span_table: RefCell::new(HashMap::new()),
            eval_depth: Cell::new(0),
            eval_step_limit: Cell::new(0),
            eval_steps: Cell::new(0),
        }
    }

    // --- File path stack ---

    pub fn push_file_path(&self, path: PathBuf) {
        self.current_file.borrow_mut().push(path);
    }

    pub fn pop_file_path(&self) {
        self.current_file.borrow_mut().pop();
    }

    pub fn current_file_dir(&self) -> Option<PathBuf> {
        self.current_file
            .borrow()
            .last()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    }

    pub fn current_file_path(&self) -> Option<PathBuf> {
        self.current_file.borrow().last().cloned()
    }

    // --- Module cache ---

    pub fn get_cached_module(&self, path: &PathBuf) -> Option<BTreeMap<String, Value>> {
        self.module_cache.borrow().get(path).cloned()
    }

    pub fn cache_module(&self, path: PathBuf, exports: BTreeMap<String, Value>) {
        self.module_cache.borrow_mut().insert(path, exports);
    }

    // --- Module exports (stack for nested imports) ---

    pub fn set_module_exports(&self, names: Vec<String>) {
        let mut stack = self.module_exports.borrow_mut();
        if let Some(top) = stack.last_mut() {
            *top = Some(names);
        }
    }

    pub fn clear_module_exports(&self) {
        self.module_exports.borrow_mut().push(None);
    }

    pub fn take_module_exports(&self) -> Option<Vec<String>> {
        self.module_exports.borrow_mut().pop().flatten()
    }

    // --- Module load stack (cyclic import detection) ---

    pub fn begin_module_load(&self, path: &PathBuf) -> Result<(), SemaError> {
        let mut stack = self.module_load_stack.borrow_mut();
        if let Some(pos) = stack.iter().position(|p| p == path) {
            let mut cycle: Vec<String> = stack[pos..]
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            cycle.push(path.display().to_string());
            return Err(SemaError::eval(format!(
                "cyclic import detected: {}",
                cycle.join(" -> ")
            )));
        }
        stack.push(path.clone());
        Ok(())
    }

    pub fn end_module_load(&self, path: &PathBuf) {
        let mut stack = self.module_load_stack.borrow_mut();
        if matches!(stack.last(), Some(last) if last == path) {
            stack.pop();
        } else if let Some(pos) = stack.iter().rposition(|p| p == path) {
            stack.remove(pos);
        }
    }

    // --- Call stack ---

    pub fn push_call_frame(&self, frame: CallFrame) {
        self.call_stack.borrow_mut().push(frame);
    }

    pub fn call_stack_depth(&self) -> usize {
        self.call_stack.borrow().len()
    }

    pub fn truncate_call_stack(&self, depth: usize) {
        self.call_stack.borrow_mut().truncate(depth);
    }

    pub fn capture_stack_trace(&self) -> StackTrace {
        StackTrace(
            self.call_stack
                .borrow()
                .iter()
                .rev()
                .cloned()
                .collect(),
        )
    }

    // --- Span table ---

    pub fn merge_span_table(&self, spans: SpanMap) {
        let mut table = self.span_table.borrow_mut();
        if table.len().saturating_add(spans.len()) > MAX_SPAN_TABLE_ENTRIES {
            table.clear();
        }
        table.extend(spans);
    }

    pub fn lookup_span(&self, ptr: usize) -> Option<Span> {
        self.span_table.borrow().get(&ptr).cloned()
    }

    // --- Eval step tracking ---

    pub fn set_eval_step_limit(&self, limit: usize) {
        self.eval_step_limit.set(limit);
    }
}

impl Default for EvalContext {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 2: Export from lib.rs**

Add `mod context;` and `pub use context::EvalContext;` to `crates/sema-core/src/lib.rs`.

**Step 3: Verify it compiles**

Run: `cargo build -p sema-core`
Expected: compiles with no errors.

**Step 4: Commit**

```
git add -A && git commit -m "feat(core): add EvalContext struct"
```

---

### Task 2: Change NativeFn signature with dual constructors

**Files:**

- Modify: `crates/sema-core/src/value.rs`

**Step 1: Write a test for both constructor styles**

Add to the bottom of `crates/sema-core/src/value.rs` (or a test module):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::EvalContext;

    #[test]
    fn test_native_fn_simple() {
        let f = NativeFn::simple("add1", |args| {
            Ok(args[0].clone())
        });
        let ctx = EvalContext::new();
        assert!((f.func)(&ctx, &[Value::Int(42)]).is_ok());
    }

    #[test]
    fn test_native_fn_with_ctx() {
        let f = NativeFn::with_ctx("get-depth", |ctx, _args| {
            Ok(Value::Int(ctx.eval_depth.get() as i64))
        });
        let ctx = EvalContext::new();
        assert_eq!((f.func)(&ctx, &[]).unwrap(), Value::Int(0));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p sema-core`
Expected: FAIL — `NativeFn::simple` and `NativeFn::with_ctx` don't exist yet.

**Step 3: Change the NativeFn type and add constructors**

In `crates/sema-core/src/value.rs`, change:

```rust
pub type NativeFnInner = dyn Fn(&EvalContext, &[Value]) -> Result<Value, SemaError>;

pub struct NativeFn {
    pub name: String,
    pub func: Box<NativeFnInner>,
}

impl NativeFn {
    /// For builtins that don't need the evaluation context.
    pub fn simple(name: impl Into<String>, f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static) -> Self {
        Self {
            name: name.into(),
            func: Box::new(move |_ctx, args| f(args)),
        }
    }

    /// For builtins that need access to evaluation context (file paths, call stack, etc.).
    pub fn with_ctx(name: impl Into<String>, f: impl Fn(&EvalContext, &[Value]) -> Result<Value, SemaError> + 'static) -> Self {
        Self {
            name: name.into(),
            func: Box::new(f),
        }
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p sema-core`
Expected: PASS

**Step 5: Commit**

```
git add -A && git commit -m "feat(core): change NativeFn to accept EvalContext, add dual constructors"
```

---

## Phase 2: Update sema-eval to thread EvalContext

### Task 3: Add EvalContext to Interpreter and thread through eval functions

**Files:**

- Modify: `crates/sema-eval/src/eval.rs`
- Modify: `crates/sema-eval/src/lib.rs`

**Step 1: Add EvalContext field to Interpreter**

In `eval.rs`, change `Interpreter`:

```rust
pub struct Interpreter {
    pub global_env: Rc<Env>,
    pub ctx: EvalContext,
}
```

Update `Interpreter::new()` to create the context. Remove the `reset_runtime_state()` call — a fresh `EvalContext` replaces it:

```rust
pub fn new() -> Self {
    let env = Env::new();
    sema_stdlib::register_stdlib(&env);
    #[cfg(not(target_arch = "wasm32"))]
    {
        sema_llm::builtins::reset_runtime_state();
        sema_llm::builtins::register_llm_builtins(&env);
    }
    let ctx = EvalContext::new();
    Interpreter {
        global_env: Rc::new(env),
        ctx,
    }
}
```

Note: `sema_llm::builtins::reset_runtime_state()` is kept for now since LLM thread-locals remain.

**Step 2: Change eval_value / eval_value_inner / eval_step signatures**

Add `ctx: &EvalContext` parameter to:

- `pub fn eval_value(expr: &Value, env: &Env, ctx: &EvalContext) -> EvalResult`
- `fn eval_value_inner(expr: &Value, env: &Env, ctx: &EvalContext) -> EvalResult`
- `fn eval_step(expr: &Value, env: &Env, ctx: &EvalContext) -> Result<Trampoline, SemaError>`
- `pub fn eval_string(input: &str, env: &Env, ctx: &EvalContext) -> EvalResult`
- `pub fn eval(expr: &Value, env: &Env, ctx: &EvalContext) -> EvalResult`

Replace all `THREAD_LOCAL.with(...)` accesses with `ctx.field` / `ctx.method()` accesses.

**Step 3: Update CallStackGuard to hold &EvalContext**

```rust
struct CallStackGuard<'a> {
    ctx: &'a EvalContext,
    entry_depth: usize,
}

impl Drop for CallStackGuard<'_> {
    fn drop(&mut self) {
        self.ctx.truncate_call_stack(self.entry_depth);
    }
}
```

**Step 4: Update span_of_expr to take &EvalContext**

```rust
fn span_of_expr(expr: &Value, ctx: &EvalContext) -> Option<Span> {
    match expr {
        Value::List(items) => {
            let ptr = Rc::as_ptr(items) as usize;
            ctx.lookup_span(ptr)
        }
        _ => None,
    }
}
```

**Step 5: Update Interpreter methods**

```rust
impl Interpreter {
    pub fn eval(&self, expr: &Value) -> EvalResult {
        eval_value(expr, &Env::with_parent(self.global_env.clone()), &self.ctx)
    }
    pub fn eval_str(&self, input: &str) -> EvalResult {
        eval_string(input, &Env::with_parent(self.global_env.clone()), &self.ctx)
    }
    pub fn eval_in_global(&self, expr: &Value) -> EvalResult {
        eval_value(expr, &self.global_env, &self.ctx)
    }
    pub fn eval_str_in_global(&self, input: &str) -> EvalResult {
        eval_string(input, &self.global_env, &self.ctx)
    }
}
```

**Step 6: Update set_eval_callback to pass ctx**

The LLM eval callback must pass the context through. Update the callback setup:

```rust
// In Interpreter::new(), after ctx is created, we need the callback to
// capture a reference. Since ctx lives on the Interpreter, the callback
// must receive ctx at call time, not capture it.
// This means EvalCallback signature must change (see Task 6).
```

**Step 7: Delete `reset_runtime_state()` from eval.rs**

This function manually cleared all thread-locals. With `EvalContext`, creating a new `Interpreter` gives fresh state automatically. Delete `reset_runtime_state` and remove it from `lib.rs` exports.

**Step 8: Verify sema-eval compiles**

Run: `cargo build -p sema-eval`
Expected: compilation errors in special_forms.rs (next task) and downstream crates (expected).

**Step 9: Commit (WIP)**

```
git add -A && git commit -m "wip(eval): thread EvalContext through eval functions"
```

---

### Task 4: Update special_forms.rs to use EvalContext

**Files:**

- Modify: `crates/sema-eval/src/special_forms.rs`

**Step 1: Add ctx parameter to try_eval_special and all special form functions**

Change the signature:

```rust
pub fn try_eval_special(
    head_spur: Spur,
    args: &[Value],
    env: &Env,
    ctx: &EvalContext,
) -> Option<Result<Trampoline, SemaError>>
```

And propagate `ctx` to every `eval_*` function in special_forms.rs (~34 functions). Each one that calls `eval::eval_value` now passes `ctx`.

**Step 2: Update eval_import to use ctx methods**

This is the most complex special form. Replace all `eval::` free function calls with `ctx.` method calls:

```rust
fn eval_import(args: &[Value], env: &Env, ctx: &EvalContext) -> Result<Trampoline, SemaError> {
    // ...
    let resolved = if std::path::Path::new(path_str).is_absolute() {
        std::path::PathBuf::from(path_str)
    } else if let Some(dir) = ctx.current_file_dir() {  // was: eval::current_file_dir()
        dir.join(path_str)
    } else {
        std::path::PathBuf::from(path_str)
    };
    // ...
    if let Some(cached) = ctx.get_cached_module(&canonical) {  // was: eval::get_cached_module
        // ...
    }

    ctx.begin_module_load(&canonical)?;  // was: eval::begin_module_load

    let load_result = (|| {
        // ...
        ctx.merge_span_table(spans);  // was: eval::merge_span_table
        let module_env = eval::create_module_env(env);
        ctx.push_file_path(canonical.clone());  // was: eval::push_file_path
        ctx.clear_module_exports();  // was: eval::clear_module_exports

        let eval_result = (|| {
            for expr in &exprs {
                eval::eval_value(expr, &module_env, ctx)?;  // added ctx
            }
            Ok(())
        })();

        ctx.pop_file_path();  // was: eval::pop_file_path
        let declared = ctx.take_module_exports();  // was: eval::take_module_exports
        eval_result?;
        Ok(collect_module_exports(&module_env, declared.as_deref()))
    })();

    ctx.end_module_load(&canonical);  // was: eval::end_module_load
    let exports = load_result?;
    ctx.cache_module(canonical, exports.clone());  // was: eval::cache_module
    // ...
}
```

**Step 3: Update eval_load similarly**

Replace `eval::push_file_path`, `eval::pop_file_path`, `eval::current_file_dir`, `eval::merge_span_table` with `ctx.` calls.

**Step 4: Update eval_module**

Replace `eval::set_module_exports(export_names)` with `ctx.set_module_exports(export_names)`.

**Step 5: Update all other special forms**

For the remaining ~30 functions, the only change is adding `, ctx` to `eval::eval_value(expr, env)` calls. This is mechanical.

**Step 6: Update call site in eval.rs**

In `eval_step`, update:

```rust
if let Some(result) = special_forms::try_eval_special(*spur, args, env, ctx) {
    return result;
}
```

**Step 7: Update native function calls in eval_step**

Where native functions are called:

```rust
match (native.func)(ctx, &eval_args) { ... }
```

And update `CallFrame` construction:

```rust
let frame = CallFrame {
    name: native.name.to_string(),
    file: ctx.current_file_path(),  // was: current_file_path()
    span: call_span,
};
```

**Step 8: Verify sema-eval compiles and tests pass**

Run: `cargo test -p sema-eval`
Expected: should compile; tests in sema-eval should pass.

**Step 9: Commit**

```
git add -A && git commit -m "feat(eval): thread EvalContext through special forms"
```

---

## Phase 3: Update stdlib and LLM native function registrations

### Task 5: Update sema-stdlib to use NativeFn::simple()

**Files:**

- Modify: `crates/sema-stdlib/src/lib.rs` (the `register_fn` helper)
- Potentially: other files if they construct `NativeFn` directly

**Step 1: Update the register_fn helper**

The stdlib has a centralized `register_fn` helper in `lib.rs`:

```rust
// Current:
Value::NativeFn(Rc::new(NativeFn {
    name: name.to_string(),
    func: Box::new(f),
}))

// Change to:
Value::NativeFn(Rc::new(NativeFn::simple(name, f)))
```

Since stdlib does NOT depend on sema-eval (no circular dep), **all stdlib functions use `NativeFn::simple()`**. No function needs `with_ctx`.

**Step 2: Also update define-record-type in special_forms.rs**

The `eval_define_record_type` special form constructs `NativeFn` directly for record constructors, predicates, and accessors. Update those 3 sites to use `NativeFn::simple()`.

**Step 3: Verify it compiles**

Run: `cargo build -p sema-stdlib`
Expected: compiles.

**Step 4: Commit**

```
git add -A && git commit -m "refactor(stdlib): use NativeFn::simple() constructor"
```

---

### Task 6: Update sema-llm builtins

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`

**Step 1: Update NativeFn construction in sema-llm**

The `register_fn` helper in builtins.rs constructs NativeFn directly. Change to use `NativeFn::simple()`.

Also update the direct `NativeFn { ... }` construction for tool handler (~line 2693) to use `NativeFn::simple()`.

**Step 2: Update the EvalCallback signature**

Change the callback to accept `&EvalContext`:

```rust
pub type EvalCallback = Box<dyn Fn(&EvalContext, &Value, &Env) -> Result<Value, SemaError>>;
```

Update `set_eval_callback`:

```rust
pub fn set_eval_callback(f: impl Fn(&EvalContext, &Value, &Env) -> Result<Value, SemaError> + 'static) {
    EVAL_FN.with(|eval| {
        *eval.borrow_mut() = Some(Box::new(f));
    });
}
```

Update `full_eval` to accept `&EvalContext`:

```rust
fn full_eval(ctx: &EvalContext, expr: &Value, env: &Env) -> Result<Value, SemaError> {
    EVAL_FN.with(|eval_fn| {
        let eval_fn = eval_fn.borrow();
        match &*eval_fn {
            Some(f) => f(ctx, expr, env),
            None => simple_eval(expr, env),
        }
    })
}
```

**Step 3: Update LLM builtins that call full_eval to use NativeFn::with_ctx()**

Any builtin that calls `full_eval` needs the context. These use `NativeFn::with_ctx()` or receive ctx through a `register_fn_ctx` helper. Check which builtins call `full_eval` and update them.

**Step 4: Keep LLM-specific thread-locals as-is**

`PROVIDER_REGISTRY`, `SESSION_USAGE`, `LAST_USAGE`, `SESSION_COST`, `BUDGET_LIMIT`, `BUDGET_SPENT`, `BUDGET_STACK`, `PRICING_WARNING_SHOWN` all remain as thread-locals. They are self-contained within sema-llm.

`reset_runtime_state()` in sema-llm remains unchanged — it resets LLM thread-locals only.

**Step 5: Verify it compiles**

Run: `cargo build -p sema-llm`

**Step 6: Commit**

```
git add -A && git commit -m "refactor(llm): update NativeFn construction and EvalCallback signature"
```

---

## Phase 4: Update top-level crates

### Task 7: Update sema binary crate (main.rs + lib.rs)

**Files:**

- Modify: `crates/sema/src/main.rs`
- Modify: `crates/sema/src/lib.rs`

**Step 1: Update lib.rs (InterpreterBuilder / Interpreter)**

In `InterpreterBuilder::build()`:

- Remove `sema_eval::reset_runtime_state()` — fresh `EvalContext` replaces it
- Keep `sema_llm::builtins::reset_runtime_state()` — LLM thread-locals still need it
- Update `set_eval_callback` to pass the new signature:

  ```rust
  sema_llm::builtins::set_eval_callback(sema_eval::eval_value);
  // eval_value now takes (&EvalContext, &Value, &Env) which matches the new EvalCallback
  ```

  Note: `eval_value` signature is `(expr: &Value, env: &Env, ctx: &EvalContext)` but `EvalCallback` expects `(&EvalContext, &Value, &Env)`. Decide parameter order to match, or use a closure wrapper.

- Update `Interpreter::register_fn()` to use `NativeFn::simple()`.

**Step 2: Update main.rs**

Replace calls to free functions:

```rust
// Before:
sema_eval::push_file_path(canonical);
sema_eval::pop_file_path();

// After:
interpreter.ctx.push_file_path(canonical);
interpreter.ctx.pop_file_path();
```

There are ~6 call sites in main.rs that use `sema_eval::push_file_path` / `sema_eval::pop_file_path` (in the load loop and file execution). Access ctx through the `sema_eval::Interpreter` which is `interpreter` in main.rs (note: main.rs uses `sema_eval::Interpreter` directly, not the public `sema::Interpreter` wrapper).

**Step 3: Verify CLI works**

Run: `cargo run -- -e "(+ 1 2)"`
Expected: `3`

Run: `cargo run -- examples/hello.sema`
Expected: runs without error.

**Step 4: Commit**

```
git add -A && git commit -m "refactor(sema): update main.rs and lib.rs for EvalContext"
```

---

### Task 8: Update WASM playground crate

**Files:**

- Modify: `playground/crate/src/lib.rs`

**Step 1: Update WasmInterpreter**

The WASM crate calls `sema_eval::eval_string(code, &env)` in two places (`eval` and `eval_global` methods). Update both to pass ctx:

```rust
// In eval():
sema_eval::eval_string(code, &env, &self.inner.ctx)

// In eval_global():
sema_eval::eval_string(code, &self.inner.global_env, &self.inner.ctx)
```

Also update `NativeFn` construction in `register_wasm_io()` (line 60):

```rust
// Current:
Value::NativeFn(Rc::new(NativeFn {
    name: name.to_string(),
    func: f,
}))

// Change to:
Value::NativeFn(Rc::new(NativeFn::simple(name, f)))
```

Note: The `register` closure in `register_wasm_io` takes `Box<dyn Fn(&[Value]) -> Result<Value, SemaError>>` — update to use `NativeFn::simple()` which wraps it. The `Box` is already the right inner type.

**Step 2: Verify it compiles**

Run: `cargo build -p sema-wasm` (requires wasm32 target; skip if not set up locally)

Alternatively verify with: `cargo check -p sema-wasm --target wasm32-unknown-unknown`

If wasm target is not installed, at minimum verify the Rust code is correct by reading through the changes.

**Step 3: Commit**

```
git add -A && git commit -m "refactor(wasm): update playground for EvalContext"
```

---

## Phase 5: Clean up and verify

### Task 9: Remove thread_local! block and old free functions from eval.rs

**Files:**

- Modify: `crates/sema-eval/src/eval.rs`
- Modify: `crates/sema-eval/src/lib.rs`

**Step 1: Delete the thread_local! block**

Remove the entire `thread_local! { ... }` macro invocation from eval.rs (currently lines 18-36, containing `MODULE_CACHE`, `CURRENT_FILE`, `MODULE_EXPORTS`, `MODULE_LOAD_STACK`, `CALL_STACK`, `SPAN_TABLE`, `EVAL_DEPTH`, `EVAL_STEP_LIMIT`, `EVAL_STEPS`).

**Step 2: Delete the old free functions**

Remove all free functions that were wrappers around thread-local access:

- `push_file_path`, `pop_file_path`, `current_file_dir`, `current_file_path`
- `get_cached_module`, `cache_module`
- `set_module_exports`, `clear_module_exports`, `take_module_exports`
- `begin_module_load`, `end_module_load`
- `push_call_frame`, `call_stack_depth`, `truncate_call_stack`, `capture_stack_trace`
- `merge_span_table`, `lookup_span`
- `set_eval_step_limit`
- `reset_runtime_state`
- `span_of_expr` (now takes `&EvalContext`, either moved to a method or kept as local fn with ctx param)

Keep `create_module_env` — it doesn't use thread-locals, just walks env parents.

**Step 3: Update lib.rs exports**

Slim down `pub use eval::{...}` to only export what's still needed:

```rust
pub use eval::{
    create_module_env, eval, eval_string, eval_value, Interpreter, EvalResult, Trampoline,
};
// Re-export EvalContext from sema-core for convenience:
pub use sema_core::EvalContext;
```

**Step 4: Verify everything compiles and all tests pass**

Run: `cargo test`
Expected: all tests pass.

**Step 5: Commit**

```
git add -A && git commit -m "refactor(eval): remove thread_local! state, all state now in EvalContext"
```

---

### Task 10: Run full test suite, lint, and manual verification

**Files:**

- Modify: `crates/sema/tests/integration_test.rs` (if needed)
- Modify: `crates/sema/tests/embedding_bench.rs` (if needed)

**Step 1: Run full test suite**

Run: `cargo test`
Expected: all tests pass. Pay special attention to:

- Module import/export tests (the nested import and cyclic detection logic changed)
- Tests that create multiple `Interpreter` instances (they now get independent state)
- The embedding bench test

**Step 2: Run clippy**

Run: `make lint`
Expected: no warnings.

**Step 3: Test module loading (import/load)**

Run: `cargo run -- examples/meta-eval.sema`
This exercises nested evaluation heavily. Verify it runs without error.

**Step 4: Test REPL**

Run: `cargo run` and try:

```
sema> (define x 42)
sema> x
42
sema> (+ x 1)
43
```

Verify define persistence works.

**Step 5: Test the meta-eval stress test**

Run: `cargo run -- examples/meta-eval-stress.sema`
This is a new test added in the recent bugfix PR that exercises nested module loading and evaluation.

**Step 6: Commit**

```
git add -A && git commit -m "chore: verify EvalContext migration passes all tests"
```

---

## Out of scope (follow-up tasks)

- **LLM thread-locals**: `PROVIDER_REGISTRY`, `SESSION_USAGE`, `SESSION_COST`, `BUDGET_LIMIT`, `BUDGET_SPENT`, `BUDGET_STACK`, `PRICING_WARNING_SHOWN` in sema-llm — keep as thread-locals for now, migrate to `LlmContext` later if needed.
- **`Send + Sync` for Interpreter**: Would require switching from `Rc` to `Arc` — much larger change.
- **Mini-evaluator in list.rs**: Currently doesn't use EvalContext. If it ever needs context access, it would need to accept `&EvalContext`. For now it's fine since it only handles simple expressions (quote, if, begin, let, lambda application).
- **Parameter order consistency**: Decide whether `eval_value` takes `(expr, env, ctx)` or `(ctx, expr, env)` — the latter matches the `NativeFnInner` signature and may be cleaner. Decide during implementation.
