use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use sema_core::{
    intern, resolve, CallFrame, Env, EvalContext, Lambda, Macro, MultiMethod, NativeFn, SemaError,
    Span, Spur, Thunk, Value, ValueView,
};

use crate::special_forms;

/// Trampoline for tail-call optimization.
pub enum Trampoline {
    Value(Value),
    Eval(Value, Env),
}

pub type EvalResult = Result<Value, SemaError>;

/// Create an isolated module env: child of root (global/stdlib) env
pub fn create_module_env(env: &Env) -> Env {
    // Walk parent chain to find root
    let mut current = env.clone();
    loop {
        let parent = current.parent.clone();
        match parent {
            Some(p) => current = (*p).clone(),
            None => break,
        }
    }
    Env::with_parent(Rc::new(current))
}

/// Look up a span for an expression via the span table in the context.
fn span_of_expr(ctx: &EvalContext, expr: &Value) -> Option<Span> {
    if let Some(items) = expr.as_list_rc() {
        let ptr = Rc::as_ptr(&items) as usize;
        ctx.lookup_span(ptr)
    } else {
        None
    }
}

/// RAII guard that truncates the call stack on drop.
struct CallStackGuard<'a> {
    ctx: &'a EvalContext,
    entry_depth: usize,
}

impl Drop for CallStackGuard<'_> {
    fn drop(&mut self) {
        self.ctx.truncate_call_stack(self.entry_depth);
    }
}

/// Collect the names of all native functions in an environment.
/// Used to tell the bytecode compiler which globals can use CallNative.
fn collect_native_names(env: &Env) -> HashSet<Spur> {
    env.all_names()
        .into_iter()
        .filter(|&spur| env.get(spur).is_some_and(|v| v.is_native_fn()))
        .collect()
}

/// The interpreter holds the global environment and state.
pub struct Interpreter {
    pub global_env: Rc<Env>,
    pub ctx: EvalContext,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        let env = Env::new();
        let ctx = EvalContext::new();
        // Register eval/call callbacks so stdlib can invoke the real evaluator
        sema_core::set_eval_callback(&ctx, eval_value);
        sema_core::set_call_callback(&ctx, call_value);
        // Register stdlib
        sema_stdlib::register_stdlib(&env, &sema_core::Sandbox::allow_all());
        // Register LLM builtins
        #[cfg(not(target_arch = "wasm32"))]
        {
            sema_llm::builtins::reset_runtime_state();
            sema_llm::builtins::register_llm_builtins(&env, &sema_core::Sandbox::allow_all());
            sema_llm::builtins::set_eval_callback(eval_value_vm);
        }
        let global_env = Rc::new(env);
        register_vm_delegates(&global_env);
        load_prelude(&ctx, &global_env);
        Interpreter { global_env, ctx }
    }

    pub fn new_with_sandbox(sandbox: &sema_core::Sandbox) -> Self {
        let env = Env::new();
        let ctx = EvalContext::new_with_sandbox(sandbox.clone());
        sema_core::set_eval_callback(&ctx, eval_value);
        sema_core::set_call_callback(&ctx, call_value);
        sema_stdlib::register_stdlib(&env, sandbox);
        #[cfg(not(target_arch = "wasm32"))]
        {
            sema_llm::builtins::reset_runtime_state();
            sema_llm::builtins::register_llm_builtins(&env, sandbox);
            sema_llm::builtins::set_eval_callback(eval_value_vm);
        }
        let global_env = Rc::new(env);
        register_vm_delegates(&global_env);
        load_prelude(&ctx, &global_env);
        Interpreter { global_env, ctx }
    }

    /// Evaluate a single expression on the VM. M6: the VM is the sole evaluator.
    ///
    /// NOTE (deliberate behavior change vs. the retired tree-walker): all eval
    /// entry points now run in the global env, so top-level `define`s persist
    /// across calls. The old `eval`/`eval_str` child-env isolation is gone —
    /// maintaining two env semantics was the dual-evaluator complexity being
    /// removed. Use a fresh `Interpreter` for an isolated evaluation.
    pub fn eval(&self, expr: &Value) -> EvalResult {
        self.eval_in_global(expr)
    }

    /// Parse and evaluate on the VM (global env; `define`s persist — see `eval`).
    pub fn eval_str(&self, input: &str) -> EvalResult {
        self.eval_str_in_global(input)
    }

    /// Evaluate in the global environment so that `define` persists across calls.
    pub fn eval_in_global(&self, expr: &Value) -> EvalResult {
        self.ctx.set_vm_backend(true);
        self.run_exprs_on_vm(std::slice::from_ref(expr), &self.global_env)
    }

    /// Parse and evaluate in the global environment so that `define` persists across calls.
    pub fn eval_str_in_global(&self, input: &str) -> EvalResult {
        self.ctx.set_vm_backend(true);
        let (exprs, spans) = sema_reader::read_many_with_spans(input)?;
        self.ctx.merge_span_table(spans);
        if exprs.is_empty() {
            return Ok(Value::nil());
        }
        self.run_exprs_on_vm(&exprs, &self.global_env)
    }

    /// Parse, compile to bytecode, and execute via the VM (global env, persists).
    pub fn eval_str_compiled(&self, input: &str) -> EvalResult {
        self.ctx.set_vm_backend(true);
        let (exprs, spans) = sema_reader::read_many_with_spans(input)?;
        self.ctx.merge_span_table(spans);
        if exprs.is_empty() {
            return Ok(Value::nil());
        }
        self.run_exprs_on_vm(&exprs, &self.global_env)
    }

    /// Macro-expand, compile, and run a sequence of top-level forms on the VM,
    /// rooted at `globals`. Shared by every eval entry point (M6: single
    /// evaluator). `define`s land in `globals`.
    fn run_exprs_on_vm(&self, exprs: &[Value], globals: &Rc<Env>) -> EvalResult {
        let mut expanded = Vec::with_capacity(exprs.len());
        for expr in exprs {
            expanded.push(expand_for_vm_in(&self.ctx, globals, expr)?);
        }
        let known_natives = collect_native_names(globals);
        let prog = sema_vm::compile_program(&expanded, Some(known_natives))?;
        let mut vm = sema_vm::VM::new(
            globals.clone(),
            prog.functions,
            &prog.native_table,
            prog.main_cache_slots,
        )?;
        sema_vm::init_scheduler(self.global_env.clone(), prog.native_table.clone());
        vm.execute(prog.closure, &self.ctx)
    }

    /// Compile source code to bytecode without executing.
    /// Handles macro expansion (defmacro + macro calls) before compilation.
    pub fn compile_to_bytecode(&self, input: &str) -> Result<sema_vm::CompileResult, SemaError> {
        let (exprs, spans) = sema_reader::read_many_with_spans(input)?;
        self.ctx.merge_span_table(spans);

        let mut expanded = Vec::new();
        for expr in &exprs {
            let exp = self.expand_for_vm(expr)?;
            if !exp.is_nil() {
                expanded.push(exp);
            }
        }

        if expanded.is_empty() {
            expanded.push(Value::nil());
        }

        let prog = sema_vm::compile_program(&expanded, None)?;
        Ok(sema_vm::CompileResult::new(
            prog.closure.func.chunk.clone(),
            prog.functions.iter().map(|f| (**f).clone()).collect(),
        ))
    }

    /// Pre-process a top-level expression for VM compilation.
    /// Evaluates `defmacro` forms via the tree-walker to register macros,
    /// then expands macro calls in all other forms.
    pub fn expand_for_vm(&self, expr: &Value) -> EvalResult {
        expand_for_vm_in(&self.ctx, &self.global_env, expr)
    }
}

/// Pre-process a top-level expression for VM compilation, expanding macro calls
/// and eagerly registering `defmacro` forms — against `env` rather than a fixed
/// global env. For top-level code `env` is the global env (unchanged behavior);
/// for a `load`ed module body it is the same shared global env, so a `defmacro`
/// registers where `expand_macros_in` looks it up and inherited macros still
/// resolve via the parent chain.
pub fn expand_for_vm_in(ctx: &EvalContext, env: &Env, expr: &Value) -> EvalResult {
    if let Some(items) = expr.as_list() {
        if let Some(s) = items.first().and_then(|v| v.as_symbol_spur()) {
            let name = resolve(s);
            if name == "defmacro" {
                // Register the macro directly (pure destructure) — the VM macro
                // path must not route through the tree-walker's `eval_value`.
                register_defmacro(items, env)?;
                return Ok(Value::nil());
            }
            if name == "begin" || name == "progn" {
                let mut new_items = vec![Value::symbol_from_spur(s)];
                let mut changed = false;
                for item in &items[1..] {
                    let expanded = expand_for_vm_in(ctx, env, item)?;
                    if expanded.raw_bits() != item.raw_bits() {
                        changed = true;
                    }
                    new_items.push(expanded);
                }
                if !changed {
                    return Ok(expr.clone());
                }
                return Ok(Value::list(new_items));
            }
        }
    }
    expand_macros_in(ctx, env, expr)
}

/// Recursively expand macro calls, resolving macros via `env` (walking the
/// parent chain). Preserves Rc pointer identity when no expansion occurs so span
/// lookups (keyed by Rc pointer) remain valid.
fn expand_macros_in(ctx: &EvalContext, env: &Env, expr: &Value) -> EvalResult {
    if let Some(items) = expr.as_list() {
        if !items.is_empty() {
            if let Some(s) = items.first().and_then(|v| v.as_symbol_spur()) {
                let name = resolve(s);
                if name == "quote" {
                    return Ok(expr.clone());
                }
                if let Some(mac_val) = env.get(s) {
                    if let Some(mac) = mac_val.as_macro_rc() {
                        // VM-native expansion: apply the transformer on the VM,
                        // not the tree-walker.
                        let expanded = apply_macro_vm(ctx, &mac, &items[1..], env)?;
                        return expand_macros_in(ctx, env, &expanded);
                    }
                }
            }
            let expanded: Vec<Value> = items
                .iter()
                .map(|v| expand_macros_in(ctx, env, v))
                .collect::<Result<_, _>>()?;
            let changed = expanded
                .iter()
                .zip(items.iter())
                .any(|(a, b)| a.raw_bits() != b.raw_bits());
            if !changed {
                return Ok(expr.clone());
            }
            return Ok(Value::list(expanded));
        }
    }
    Ok(expr.clone())
}

/// Compile and run a `load`ed module body on the VM, one top-level form at a
/// time so a `defmacro` / nested `load` that registers a macro is visible to
/// later forms before they compile. `env` is the caller's shared global env, so
/// defines land in the global scope (matching `load` semantics). Returns the
/// value of the last form (nil for an empty body).
///
/// Only used for `load` (not `import`): `load` shares the global env, so module
/// functions resolve their globals against the same env every VM uses — avoiding
/// the per-module-globals problem that makes VM-backed `import` incorrect (see
/// docs/plans/2026-06-16-vm-module-loading.md). Does NOT (re)initialize the async
/// scheduler — it reuses the one installed by the top-level VM driver.
pub fn eval_module_body_vm(
    ctx: &EvalContext,
    env: &Env,
    exprs: &[Value],
    span_map: &sema_core::SpanMap,
    source_file: Option<std::path::PathBuf>,
) -> EvalResult {
    let mut result = Value::nil();
    for expr in exprs {
        let expanded = expand_for_vm_in(ctx, env, expr)?;
        // `defmacro` (and forms that expand to nothing) are applied by expansion;
        // there is nothing to compile/run for them.
        if expanded.is_nil() {
            continue;
        }
        let prog = sema_vm::compile_program_with_spans(
            std::slice::from_ref(&expanded),
            span_map,
            source_file.clone(),
        )?;
        let globals = Rc::new(env.clone());
        let mut vm = sema_vm::VM::new(
            globals,
            prog.functions,
            &prog.native_table,
            prog.main_cache_slots,
        )?;
        result = vm.execute(prog.closure, ctx)?;
    }
    // Each per-form VM ran on a clone of `env` with its own version cell, so any
    // globals (re)defined by the body did not bump `env`'s version. Bump it now
    // so the calling VM (whose globals share `env`'s bindings) invalidates its
    // inline global cache and re-reads, rather than serving stale cached values.
    env.bump_version();
    Ok(result)
}

/// Evaluate a string containing one or more expressions.
pub fn eval_string(ctx: &EvalContext, input: &str, env: &Env) -> EvalResult {
    // Tree-walker entry point: loaded bodies are tree-walked.
    ctx.set_vm_backend(false);
    let (exprs, spans) = sema_reader::read_many_with_spans(input)?;
    ctx.merge_span_table(spans);
    ctx.max_eval_depth.set(0);
    let mut result = Value::nil();
    for expr in &exprs {
        result = eval_value(ctx, expr, env)?;
    }
    Ok(result)
}

/// The core eval function: evaluate a Value in an environment.
pub fn eval(ctx: &EvalContext, expr: &Value, env: &Env) -> EvalResult {
    eval_value(ctx, expr, env)
}

/// Maximum eval nesting depth before we bail with an error.
/// This prevents native stack overflow from unbounded recursion
/// (both function calls and special form nesting like deeply nested if/let/begin).
/// WASM has a much smaller call stack (~1MB V8 limit) so we use a lower depth.
#[cfg(target_arch = "wasm32")]
const MAX_EVAL_DEPTH: usize = 256;
#[cfg(not(target_arch = "wasm32"))]
const MAX_EVAL_DEPTH: usize = 1024;

pub fn eval_value(ctx: &EvalContext, expr: &Value, env: &Env) -> EvalResult {
    // Fast path: self-evaluating forms skip depth/step tracking entirely.
    match expr.view() {
        ValueView::Nil
        | ValueView::Bool(_)
        | ValueView::Int(_)
        | ValueView::Float(_)
        | ValueView::String(_)
        | ValueView::Char(_)
        | ValueView::Keyword(_)
        | ValueView::Thunk(_)
        | ValueView::Bytevector(_)
        | ValueView::NativeFn(_)
        | ValueView::Lambda(_)
        | ValueView::HashMap(_) => return Ok(expr.clone()),
        ValueView::Symbol(spur) => {
            if let Some(val) = env.get(spur) {
                return Ok(val);
            }
            let name = resolve(spur);
            let mut err = SemaError::Unbound(name.clone());
            // Check for common names from other Lisp dialects first
            if let Some(hint) = sema_core::error::veteran_hint(&name) {
                err = err.with_hint(hint);
            } else {
                // Fall back to fuzzy matching
                let all_names: Vec<String> = env.all_names().iter().map(|s| resolve(*s)).collect();
                let candidates: Vec<&str> = all_names.iter().map(|s| s.as_str()).collect();
                if let Some(suggestion) = sema_core::error::suggest_similar(&name, &candidates) {
                    err = err.with_hint(format!("Did you mean '{suggestion}'?"));
                }
            }
            let trace = ctx.capture_stack_trace();
            return Err(err.with_stack_trace(trace));
        }
        _ => {}
    }

    let depth = ctx.eval_depth.get();
    ctx.eval_depth.set(depth + 1);
    if depth + 1 > ctx.max_eval_depth.get() {
        ctx.max_eval_depth.set(depth + 1);
    }
    if depth == 0 {
        ctx.eval_steps.set(0);
    }
    if depth > MAX_EVAL_DEPTH {
        ctx.eval_depth.set(ctx.eval_depth.get().saturating_sub(1));
        return Err(SemaError::eval(format!(
            "maximum eval depth exceeded ({MAX_EVAL_DEPTH})"
        )).with_hint("this usually means infinite recursion; ensure recursive calls are in tail position for TCO, or use 'do' for iteration"));
    }

    let result = eval_value_inner(ctx, expr, env);

    ctx.eval_depth.set(ctx.eval_depth.get().saturating_sub(1));
    result
}

/// VM-native evaluation for callback consumers (e.g. sema-llm tool handlers):
/// macro-expand, compile, and run `expr` on a fresh bytecode VM rooted at `env`.
/// This is the VM-backed counterpart of `eval_value`, used to keep the
/// eval-callback path off the tree-walker (M5 / Phase 1c). Each call builds a
/// throwaway VM over a clone of `env` (sharing its bindings), so it is suited to
/// one-shot evaluation rather than a persistent define-accumulating session.
pub fn eval_value_vm(ctx: &EvalContext, expr: &Value, env: &Env) -> EvalResult {
    let env_rc = Rc::new(env.clone());
    let expanded = expand_for_vm_in(ctx, &env_rc, expr)?;
    if expanded.is_nil() {
        return Ok(Value::nil());
    }
    let prog = sema_vm::compile_program(std::slice::from_ref(&expanded), None)?;
    let mut vm = sema_vm::VM::new(env_rc, prog.functions, &[], prog.main_cache_slots)?;
    vm.execute(prog.closure, ctx)
}

/// Call a function value with already-evaluated arguments.
/// This is the public API for stdlib functions that need to invoke callbacks.
///
/// For lambdas, this delegates to `apply_lambda` + a trampoline loop so that
/// subsequent evaluation happens iteratively rather than adding Rust stack
/// frames.  This is critical for WASM where the call stack is limited (~5 MB).
pub fn call_value(ctx: &EvalContext, func: &Value, args: &[Value]) -> EvalResult {
    match func.view() {
        ValueView::NativeFn(native) => (native.func)(ctx, args),
        ValueView::Lambda(lambda) => {
            let trampoline = apply_lambda(ctx, &lambda, args)?;
            run_trampoline(ctx, trampoline)
        }
        ValueView::Keyword(spur) => {
            if args.len() != 1 {
                let name = resolve(spur);
                return Err(SemaError::arity(format!(":{name}"), "1", args.len()));
            }
            let key = Value::keyword_from_spur(spur);
            match args[0].view() {
                ValueView::Map(map) => Ok(map.get(&key).cloned().unwrap_or(Value::nil())),
                ValueView::HashMap(map) => Ok(map.get(&key).cloned().unwrap_or(Value::nil())),
                _ => Err(SemaError::type_error_with_value(
                    "map",
                    args[0].type_name(),
                    &args[0],
                )),
            }
        }
        ValueView::MultiMethod(mm) => call_multimethod(ctx, &mm, args),
        _ => Err(
            SemaError::eval(format!("not callable: {} ({})", func, func.type_name()))
                .with_hint("expected a function, lambda, or keyword"),
        ),
    }
}

/// Call a multimethod: dispatch on args, look up handler, call it.
fn call_multimethod(ctx: &EvalContext, mm: &Rc<MultiMethod>, args: &[Value]) -> EvalResult {
    let dispatch_val = call_value(ctx, &mm.dispatch_fn, args)?;
    let methods = mm.methods.borrow();
    if let Some(handler) = methods.get(&dispatch_val) {
        let handler = handler.clone();
        drop(methods);
        call_value(ctx, &handler, args)
    } else {
        drop(methods);
        let default = mm.default.borrow().clone();
        if let Some(handler) = default {
            call_value(ctx, &handler, args)
        } else {
            Err(SemaError::eval(format!(
                "no method in multimethod '{}' for dispatch value: {}",
                resolve(mm.name),
                dispatch_val
            ))
            .with_hint("add a (defmethod name :default handler) to handle unmatched values"))
        }
    }
}

/// Run a trampoline to completion iteratively.
/// Used by `call_value` so that stdlib HOF callbacks (map, for-each, etc.)
/// don't grow the Rust call stack for every evaluation step.
fn run_trampoline(ctx: &EvalContext, trampoline: Trampoline) -> EvalResult {
    let limit = ctx.eval_step_limit.get();
    let has_deadline = ctx.eval_deadline.get().is_some();
    let mut current = trampoline;
    let mut deadline_tick: u32 = 0;
    loop {
        match current {
            Trampoline::Value(v) => return Ok(v),
            Trampoline::Eval(expr, env) => {
                if limit > 0 {
                    let v = ctx.eval_steps.get() + 1;
                    ctx.eval_steps.set(v);
                    if v > limit {
                        return Err(SemaError::eval("eval step limit exceeded".to_string()));
                    }
                }
                // Wall-clock deadline check (sampled every 1024 steps to keep cost low)
                if has_deadline {
                    deadline_tick = deadline_tick.wrapping_add(1);
                    if (deadline_tick & 0x3FF) == 0 {
                        ctx.check_deadline()?;
                    }
                }
                match eval_step(ctx, &expr, &env) {
                    Ok(t) => current = t,
                    Err(e) => {
                        if e.stack_trace().is_none() {
                            let trace = ctx.capture_stack_trace();
                            return Err(e.with_stack_trace(trace));
                        }
                        return Err(e);
                    }
                }
            }
        }
    }
}

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
            Ok(v)
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

            let has_deadline = ctx.eval_deadline.get().is_some();
            let mut deadline_tick: u32 = 0;
            loop {
                if limit > 0 {
                    let v = ctx.eval_steps.get() + 1;
                    ctx.eval_steps.set(v);
                    if v > limit {
                        return Err(SemaError::eval("eval step limit exceeded".to_string()));
                    }
                }
                if has_deadline {
                    deadline_tick = deadline_tick.wrapping_add(1);
                    if (deadline_tick & 0x3FF) == 0 && ctx.deadline_exceeded() {
                        drop(guard);
                        return Err(SemaError::eval(
                            "evaluation exceeded time budget (looks like an infinite loop?)"
                                .to_string(),
                        ));
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
            Err(e)
        }
    }
}

fn eval_step(ctx: &EvalContext, expr: &Value, env: &Env) -> Result<Trampoline, SemaError> {
    match expr.view() {
        // Self-evaluating forms
        ValueView::Nil
        | ValueView::Bool(_)
        | ValueView::Int(_)
        | ValueView::Float(_)
        | ValueView::String(_)
        | ValueView::Char(_)
        | ValueView::Thunk(_)
        | ValueView::Bytevector(_) => Ok(Trampoline::Value(expr.clone())),
        ValueView::Keyword(_) => Ok(Trampoline::Value(expr.clone())),
        ValueView::Vector(items) => {
            let mut result = Vec::with_capacity(items.len());
            for item in items.iter() {
                result.push(eval_value(ctx, item, env)?);
            }
            Ok(Trampoline::Value(Value::vector(result)))
        }
        ValueView::Map(map) => {
            let mut result = std::collections::BTreeMap::new();
            for (k, v) in map.iter() {
                let ek = eval_value(ctx, k, env)?;
                let ev = eval_value(ctx, v, env)?;
                result.insert(ek, ev);
            }
            Ok(Trampoline::Value(Value::map(result)))
        }
        ValueView::HashMap(_) => Ok(Trampoline::Value(expr.clone())),

        // Symbol lookup
        ValueView::Symbol(spur) => env.get(spur).map(Trampoline::Value).ok_or_else(|| {
            let name = resolve(spur);
            let mut err = SemaError::Unbound(name.clone());
            if let Some(hint) = sema_core::error::veteran_hint(&name) {
                err = err.with_hint(hint);
            } else {
                let all_names: Vec<String> = env.all_names().iter().map(|s| resolve(*s)).collect();
                let candidates: Vec<&str> = all_names.iter().map(|s| s.as_str()).collect();
                if let Some(suggestion) = sema_core::error::suggest_similar(&name, &candidates) {
                    err = err.with_hint(format!("Did you mean '{suggestion}'?"));
                }
            }
            err
        }),

        // Function application / special forms
        ValueView::List(items) => {
            if items.is_empty() {
                return Ok(Trampoline::Value(Value::nil()));
            }

            let head = &items[0];
            let args = &items[1..];

            // O(1) special form dispatch: compare the symbol's Spur (u32 interned handle)
            // against cached constants, avoiding string resolution entirely.
            if let Some(spur) = head.as_symbol_spur() {
                if let Some(result) = special_forms::try_eval_special(spur, args, env, ctx) {
                    return result;
                }
            }

            // Evaluate the head to get the callable
            let func = eval_value(ctx, head, env)?;

            // Look up the span of the call site expression
            let call_span = span_of_expr(ctx, expr);

            match func.view() {
                ValueView::NativeFn(native) => {
                    // Evaluate arguments
                    let mut eval_args = Vec::with_capacity(args.len());
                    for arg in args {
                        eval_args.push(eval_value(ctx, arg, env)?);
                    }
                    // Push frame, call native fn
                    let frame = CallFrame {
                        name: native.name.to_string(),
                        file: ctx.current_file_path(),
                        span: call_span,
                    };
                    ctx.push_call_frame(frame);
                    match (native.func)(ctx, &eval_args) {
                        Ok(v) => {
                            // Pop on success (native fns don't trampoline)
                            ctx.truncate_call_stack(ctx.call_stack_depth().saturating_sub(1));
                            Ok(Trampoline::Value(v))
                        }
                        // On error, leave frame for stack trace capture
                        Err(e) => Err(annotate_arity_error(e, expr)),
                    }
                }
                ValueView::Lambda(lambda) => {
                    // Evaluate arguments
                    let mut eval_args = Vec::with_capacity(args.len());
                    for arg in args {
                        eval_args.push(eval_value(ctx, arg, env)?);
                    }
                    // Push frame — trampoline continues, eval_value guard handles cleanup
                    let frame = CallFrame {
                        name: lambda
                            .name
                            .map(resolve)
                            .unwrap_or_else(|| "<lambda>".to_string()),
                        file: ctx.current_file_path(),
                        span: call_span,
                    };
                    ctx.push_call_frame(frame);
                    apply_lambda(ctx, &lambda, &eval_args)
                        .map_err(|e| annotate_arity_error(e, expr))
                }
                ValueView::Macro(mac) => {
                    // Macros receive unevaluated arguments
                    let expanded = apply_macro(ctx, &mac, args, env)?;
                    // Evaluate the expansion in the current env (TCO)
                    Ok(Trampoline::Eval(expanded, env.clone()))
                }
                ValueView::Keyword(spur) => {
                    // Keywords as functions: (:key map) => (get map :key)
                    if args.len() != 1 {
                        let name = resolve(spur);
                        return Err(SemaError::arity(format!(":{name}"), "1", args.len()));
                    }
                    let map_val = eval_value(ctx, &args[0], env)?;
                    let key = Value::keyword_from_spur(spur);
                    match map_val.view() {
                        ValueView::Map(map) => Ok(Trampoline::Value(
                            map.get(&key).cloned().unwrap_or(Value::nil()),
                        )),
                        ValueView::HashMap(map) => Ok(Trampoline::Value(
                            map.get(&key).cloned().unwrap_or(Value::nil()),
                        )),
                        _ => Err(SemaError::type_error_with_value(
                            "map",
                            map_val.type_name(),
                            &map_val,
                        )),
                    }
                }
                ValueView::MultiMethod(mm) => {
                    let mut eval_args = Vec::with_capacity(args.len());
                    for arg in args {
                        eval_args.push(eval_value(ctx, arg, env)?);
                    }
                    let result = call_multimethod(ctx, &mm, &eval_args)?;
                    Ok(Trampoline::Value(result))
                }
                _ => Err(
                    SemaError::eval(format!("not callable: {} ({})", func, func.type_name()))
                        .with_hint("the first element of a list must be a function or macro"),
                ),
            }
        }

        _other => Ok(Trampoline::Value(expr.clone())),
    }
}

/// If `err` is an arity error, attach a note showing the original call form.
fn annotate_arity_error(err: SemaError, expr: &Value) -> SemaError {
    if matches!(err.inner(), SemaError::Arity { .. }) && err.note().is_none() {
        let form_str = format!("{}", expr);
        // Truncate on a char boundary (CORE-1): byte-slicing &form_str[..79]
        // panics when byte 79 lands inside a multibyte UTF-8 char.
        let truncated = match form_str.char_indices().nth(79) {
            Some((idx, _)) => format!("{}…", &form_str[..idx]),
            None => form_str,
        };
        err.with_note(format!("in: {truncated}"))
    } else {
        err
    }
}

/// Apply a lambda to evaluated arguments with TCO.
fn apply_lambda(
    ctx: &EvalContext,
    lambda: &Rc<Lambda>,
    args: &[Value],
) -> Result<Trampoline, SemaError> {
    let new_env = Env::with_parent(Rc::new(lambda.env.clone()));

    // Bind parameters
    if let Some(rest) = lambda.rest_param {
        if args.len() < lambda.params.len() {
            return Err(SemaError::arity(
                lambda
                    .name
                    .map(resolve)
                    .unwrap_or_else(|| "lambda".to_string()),
                format!("{}+", lambda.params.len()),
                args.len(),
            ));
        }
        for (param, arg) in lambda.params.iter().zip(args.iter()) {
            new_env.set(*param, arg.clone());
        }
        let rest_args = args[lambda.params.len()..].to_vec();
        new_env.set(rest, Value::list(rest_args));
    } else {
        if args.len() != lambda.params.len() {
            return Err(SemaError::arity(
                lambda
                    .name
                    .map(resolve)
                    .unwrap_or_else(|| "lambda".to_string()),
                lambda.params.len().to_string(),
                args.len(),
            ));
        }
        for (param, arg) in lambda.params.iter().zip(args.iter()) {
            new_env.set(*param, arg.clone());
        }
    }

    // Self-reference for recursion — just clone the Rc pointer
    if let Some(name) = lambda.name {
        new_env.set(name, Value::lambda_from_rc(Rc::clone(lambda)));
    }

    // Evaluate body with TCO on last expression
    if lambda.body.is_empty() {
        return Ok(Trampoline::Value(Value::nil()));
    }
    for expr in &lambda.body[..lambda.body.len() - 1] {
        eval_value(ctx, expr, &new_env)?;
    }
    Ok(Trampoline::Eval(
        lambda.body.last().unwrap().clone(),
        new_env,
    ))
}

/// Apply a macro: bind unevaluated args, evaluate body to produce expansion.
pub fn apply_macro(
    ctx: &EvalContext,
    mac: &sema_core::Macro,
    args: &[Value],
    caller_env: &Env,
) -> Result<Value, SemaError> {
    let env = Env::with_parent(Rc::new(caller_env.clone()));

    // Bind parameters to unevaluated forms
    if let Some(rest) = mac.rest_param {
        if args.len() < mac.params.len() {
            return Err(SemaError::arity(
                resolve(mac.name),
                format!("{}+", mac.params.len()),
                args.len(),
            ));
        }
        for (param, arg) in mac.params.iter().zip(args.iter()) {
            env.set(*param, arg.clone());
        }
        let rest_args = args[mac.params.len()..].to_vec();
        env.set(rest, Value::list(rest_args));
    } else {
        if args.len() != mac.params.len() {
            return Err(SemaError::arity(
                resolve(mac.name),
                mac.params.len().to_string(),
                args.len(),
            ));
        }
        for (param, arg) in mac.params.iter().zip(args.iter()) {
            env.set(*param, arg.clone());
        }
    }

    // Evaluate the macro body to get the expansion
    let mut result = Value::nil();
    for expr in &mac.body {
        result = eval_value(ctx, expr, &env)?;
    }
    Ok(result)
}

/// Apply a macro by evaluating its body on the **bytecode VM** (no tree-walker).
///
/// This is the VM-native counterpart of [`apply_macro`]. The macro's
/// (unevaluated) arguments are bound — together with a possible rest list — as
/// *globals* in a transient child env of `caller_env`; the transformer body is
/// then compiled fresh per call site (so auto-gensym stays hygienic — a cached
/// transformer would reuse the same gensym across call sites) and run on a VM
/// rooted at that env. Rooting at `caller_env` lets transformer bodies call
/// global helpers and reference module-level bindings, and binding params as
/// globals lets the compiled body resolve them via `GetGlobal`.
///
/// Used by the VM macro pre-expansion path (`expand_macros_in`) and
/// `__vm-macroexpand`. The tree-walker's own lazy expansion keeps using
/// [`apply_macro`] until the tree-walker is retired.
pub fn apply_macro_vm(
    ctx: &EvalContext,
    mac: &sema_core::Macro,
    args: &[Value],
    caller_env: &Env,
) -> Result<Value, SemaError> {
    let env = Rc::new(Env::with_parent(Rc::new(caller_env.clone())));

    // Bind parameters to unevaluated forms (same arity rules as apply_macro).
    if let Some(rest) = mac.rest_param {
        if args.len() < mac.params.len() {
            return Err(SemaError::arity(
                resolve(mac.name),
                format!("{}+", mac.params.len()),
                args.len(),
            ));
        }
        for (param, arg) in mac.params.iter().zip(args.iter()) {
            env.set(*param, arg.clone());
        }
        env.set(rest, Value::list(args[mac.params.len()..].to_vec()));
    } else {
        if args.len() != mac.params.len() {
            return Err(SemaError::arity(
                resolve(mac.name),
                mac.params.len().to_string(),
                args.len(),
            ));
        }
        for (param, arg) in mac.params.iter().zip(args.iter()) {
            env.set(*param, arg.clone());
        }
    }

    // Compile and run each body form on the VM, fresh per call site (no cache)
    // to keep auto-gensym hygienic. The body is the *transformer* code; it is
    // NOT macro-pre-expanded here — quasiquote templates inside it (which may
    // legitimately mention the macro's own name, as the recursive threading
    // macros do) must be compiled as data, not re-expanded. Any macro call the
    // transformer *produces* is re-expanded by the caller (`expand_macros_in`
    // recurses on the returned form). `compile_program` lowers quasiquote /
    // unquote / unquote-splicing directly, matching the tree-walker's
    // `eval_value` over the same body.
    let mut result = Value::nil();
    for expr in &mac.body {
        let prog = sema_vm::compile_program(std::slice::from_ref(expr), None)?;
        let mut vm = sema_vm::VM::new(env.clone(), prog.functions, &[], prog.main_cache_slots)?;
        result = vm.execute(prog.closure, ctx)?;
    }
    Ok(result)
}

/// Register a `defmacro` form's macro in `env` **without** the tree-walker — a
/// pure destructure mirroring `special_forms::eval_defmacro`. Used by the VM
/// pre-expansion path so registering a macro never routes through `eval_value`.
fn register_defmacro(items: &[Value], env: &Env) -> Result<(), SemaError> {
    // items[0] is the `defmacro` symbol; the rest are name, params, body…
    let args = &items[1..];
    if args.len() < 3 {
        return Err(SemaError::arity("defmacro", "3+", args.len()));
    }
    let name_spur = args[0]
        .as_symbol_spur()
        .ok_or_else(|| SemaError::eval("defmacro: name must be a symbol"))?;
    let param_list = args[1]
        .as_list()
        .ok_or_else(|| SemaError::eval("defmacro: params must be a list"))?;
    let param_names: Vec<sema_core::Spur> = param_list
        .iter()
        .map(|v| {
            v.as_symbol_spur()
                .ok_or_else(|| SemaError::eval("defmacro: parameter must be a symbol"))
        })
        .collect::<Result<_, _>>()?;
    let (params, rest_param) = special_forms::parse_params(&param_names);
    let body = args[2..].to_vec();
    env.set(
        name_spur,
        Value::macro_val(Macro {
            params,
            rest_param,
            body,
            name: name_spur,
        }),
    );
    Ok(())
}

/// Register `__vm-*` native functions that the bytecode VM calls back into
/// the tree-walker for forms that cannot be fully compiled.
/// Load built-in macros (threading, when-let, if-let) into the global environment.
fn load_prelude(ctx: &EvalContext, env: &Rc<Env>) {
    let exprs = sema_reader::read_many(crate::prelude::PRELUDE)
        .unwrap_or_else(|e| panic!("internal: prelude failed to parse: {e}"));
    // The prelude is exclusively `defmacro` forms. Register them via the
    // VM-native pre-expansion path so prelude loading never routes macro
    // registration through the tree-walker's `eval_value`.
    for expr in &exprs {
        expand_for_vm_in(ctx, env, expr)
            .unwrap_or_else(|e| panic!("internal: prelude failed to load: {e}"));
    }
}

fn register_vm_delegates(env: &Rc<Env>) {
    // __vm-eval: macro-expand, compile, and run the expression on the bytecode
    // VM (rooted at the global env so top-level `define`s persist). The runtime
    // `(eval ...)` meta path is thus VM-native — it no longer round-trips
    // through the tree-walker's `eval_value` (M3 / Phase 1c).
    let eval_env = env.clone();
    env.set(
        intern("__vm-eval"),
        Value::native_fn(NativeFn::with_ctx("__vm-eval", move |ctx, args| {
            if args.len() != 1 {
                return Err(SemaError::arity("eval", "1", args.len()));
            }
            let expanded = expand_for_vm_in(ctx, &eval_env, &args[0])?;
            // A form that expands to nothing (e.g. a `defmacro`) yields nil.
            if expanded.is_nil() {
                return Ok(Value::nil());
            }
            let prog = sema_vm::compile_program(std::slice::from_ref(&expanded), None)?;
            let mut vm =
                sema_vm::VM::new(eval_env.clone(), prog.functions, &[], prog.main_cache_slots)?;
            vm.execute(prog.closure, ctx)
        })),
    );

    // __vm-module-exports: register a `(module name (export ...) ...)` form's
    // declared export list with the active module-load scope, so `import`
    // restricts the copied bindings to exactly those names. Without this the VM
    // exported every top-level binding (private helpers leaked). Mirrors the
    // tree-walker's `set_module_exports` call in eval_module.
    env.set(
        intern("__vm-module-exports"),
        Value::native_fn(NativeFn::with_ctx(
            "__vm-module-exports",
            move |ctx, args| {
                if args.len() != 1 {
                    return Err(SemaError::arity("module-exports", "1", args.len()));
                }
                let names: Vec<String> = match args[0].as_list() {
                    Some(items) => items
                        .iter()
                        .map(|v| {
                            v.as_symbol().map(|s| s.to_string()).ok_or_else(|| {
                                SemaError::eval("module: export names must be symbols")
                            })
                        })
                        .collect::<Result<_, _>>()?,
                    None => return Err(SemaError::type_error("list", args[0].type_name())),
                };
                ctx.set_module_exports(names);
                Ok(Value::nil())
            },
        )),
    );

    // __vm-load: call the load driver (special_forms::eval_load) directly, not
    // through the tree-walker's eval_step dispatch. The driver handles VFS
    // resolution, file path push/pop, caching, and runs the loaded body on the
    // VM (M4). The path arrives already evaluated from the VM.
    let load_env = env.clone();
    env.set(
        intern("__vm-load"),
        Value::native_fn(NativeFn::with_ctx("__vm-load", move |ctx, args| {
            if args.len() != 1 {
                return Err(SemaError::arity("load", "1", args.len()));
            }
            // Target the *currently executing* VM's env (the module being run),
            // falling back to the global env at top level, so a nested `load`
            // adds definitions to the right module env — not always the globals.
            let target = sema_vm::current_vm_globals().unwrap_or_else(|| load_env.clone());
            match special_forms::eval_load(std::slice::from_ref(&args[0]), &target, ctx)? {
                Trampoline::Value(v) => Ok(v),
                Trampoline::Eval(..) => Ok(Value::nil()),
            }
        })),
    );

    // __vm-import: call the import driver (special_forms::eval_import) directly,
    // not through the tree-walker's eval_step dispatch. Under the VM backend the
    // driver compiles and runs the module body on the VM (M4). The path and
    // selective-import symbols arrive already evaluated from the VM.
    let import_env = env.clone();
    env.set(
        intern("__vm-import"),
        Value::native_fn(NativeFn::with_ctx("__vm-import", move |ctx, args| {
            if args.len() != 2 {
                return Err(SemaError::arity("import", "2", args.len()));
            }
            ctx.sandbox.check(sema_core::Caps::FS_READ, "import")?;
            let mut imp_args = vec![args[0].clone()];
            if let Some(items) = args[1].as_list() {
                imp_args.extend(items.iter().cloned());
            }
            // Copy exports into the *currently executing* VM's env (the module
            // being run), falling back to the global env at top level. This keeps
            // a nested module's imports private to that module instead of leaking
            // into the global env (M4 nested-module isolation).
            let target = sema_vm::current_vm_globals().unwrap_or_else(|| import_env.clone());
            match special_forms::eval_import(&imp_args, &target, ctx)? {
                Trampoline::Value(v) => Ok(v),
                Trampoline::Eval(..) => Ok(Value::nil()),
            }
        })),
    );

    // __vm-defmacro: register a macro in the environment
    let macro_env = env.clone();
    env.set(
        intern("__vm-defmacro"),
        Value::native_fn(NativeFn::simple("__vm-defmacro", move |args| {
            if args.len() != 4 {
                return Err(SemaError::arity("defmacro", "4", args.len()));
            }
            let name = match args[0].as_symbol_spur() {
                Some(s) => s,
                None => return Err(SemaError::type_error("symbol", args[0].type_name())),
            };
            let params = match args[1].as_list() {
                Some(items) => items
                    .iter()
                    .map(|v| match v.as_symbol_spur() {
                        Some(s) => Ok(s),
                        None => Err(SemaError::type_error("symbol", v.type_name())),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                None => return Err(SemaError::type_error("list", args[1].type_name())),
            };
            let rest_param = if let Some(s) = args[2].as_symbol_spur() {
                Some(s)
            } else if args[2].is_nil() {
                None
            } else {
                return Err(SemaError::type_error("symbol or nil", args[2].type_name()));
            };
            let body = vec![args[3].clone()];
            macro_env.set(
                name,
                Value::macro_val(Macro {
                    params,
                    rest_param,
                    body,
                    name,
                }),
            );
            Ok(Value::nil())
        })),
    );

    // __vm-defmacro-form: register a complete `(defmacro ...)` form directly
    // (pure destructure) — no tree-walker round-trip. Used for defmacro that
    // reaches compilation (e.g. non-top-level) rather than expand-time
    // registration.
    let dmf_env = env.clone();
    env.set(
        intern("__vm-defmacro-form"),
        Value::native_fn(NativeFn::simple("__vm-defmacro-form", move |args| {
            if args.len() != 1 {
                return Err(SemaError::arity("defmacro-form", "1", args.len()));
            }
            let items = args[0]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;
            register_defmacro(items, &dmf_env)?;
            Ok(Value::nil())
        })),
    );

    // __vm-define-record-type: delegate to the tree-walker
    let drt_env = env.clone();
    env.set(
        intern("__vm-define-record-type"),
        Value::native_fn(NativeFn::simple("__vm-define-record-type", move |args| {
            if args.len() != 5 {
                return Err(SemaError::arity("define-record-type", "5", args.len()));
            }
            // Build the `(define-record-type ...)` argument list (without the head
            // symbol) and register the type directly via the pure destructure —
            // no tree-walker round-trip. eval_define_record_type only sets native
            // ctor/predicate/accessor fns in the env; it evaluates no user code.
            let mut ctor_form = vec![args[1].clone()];
            if let Some(fields) = args[3].as_list() {
                ctor_form.extend(fields.iter().cloned());
            }
            let mut dr_args = vec![args[0].clone(), Value::list(ctor_form), args[2].clone()];
            if let Some(specs) = args[4].as_list() {
                for spec in specs.iter() {
                    dr_args.push(spec.clone());
                }
            }
            match special_forms::eval_define_record_type(&dr_args, &drt_env)? {
                Trampoline::Value(v) => Ok(v),
                Trampoline::Eval(..) => Ok(Value::nil()),
            }
        })),
    );

    // __vm-delay: create a thunk with unevaluated body
    env.set(
        intern("__vm-delay"),
        Value::native_fn(NativeFn::simple("__vm-delay", |args| {
            if args.len() != 1 {
                return Err(SemaError::arity("delay", "1", args.len()));
            }
            // args[0] is the unevaluated body expression (passed as a quoted constant)
            Ok(Value::thunk(Thunk {
                body: args[0].clone(),
                forced: RefCell::new(None),
            }))
        })),
    );

    // __vm-force: force a thunk
    let force_env = env.clone();
    env.set(
        intern("__vm-force"),
        Value::native_fn(NativeFn::with_ctx("__vm-force", move |ctx, args| {
            if args.len() != 1 {
                return Err(SemaError::arity("force", "1", args.len()));
            }
            if let Some(thunk) = args[0].as_thunk_rc() {
                if let Some(val) = thunk.forced.borrow().as_ref() {
                    return Ok(val.clone());
                }
                let val = if thunk.body.as_native_fn_rc().is_some()
                    || thunk.body.as_lambda_rc().is_some()
                {
                    sema_core::call_callback(ctx, &thunk.body, &[])?
                } else {
                    sema_core::eval_callback(ctx, &thunk.body, &force_env)?
                };
                *thunk.forced.borrow_mut() = Some(val.clone());
                Ok(val)
            } else {
                Err(SemaError::type_error("thunk", args[0].type_name())
                    .with_hint("force: argument must be a (delay ...) or promise — non-promise values are an error"))
            }
        })),
    );

    // __vm-macroexpand: expand a macro form via the tree-walker
    let me_env = env.clone();
    env.set(
        intern("__vm-macroexpand"),
        Value::native_fn(NativeFn::with_ctx("__vm-macroexpand", move |ctx, args| {
            if args.len() != 1 {
                return Err(SemaError::arity("macroexpand", "1", args.len()));
            }
            if let Some(items) = args[0].as_list() {
                if !items.is_empty() {
                    if let Some(spur) = items[0].as_symbol_spur() {
                        if let Some(mac_val) = me_env.get(spur) {
                            if let Some(mac) = mac_val.as_macro_rc() {
                                // VM-native: expand the transformer on the VM.
                                return apply_macro_vm(ctx, &mac, &items[1..], &me_env);
                            }
                        }
                    }
                }
            }
            Ok(args[0].clone())
        })),
    );

    // __vm-prompt: build Prompt directly from pre-evaluated entries
    env.set(
        intern("__vm-prompt"),
        Value::native_fn(NativeFn::simple("__vm-prompt", |args| {
            use sema_core::{Message, Prompt, Role};
            if args.len() != 1 {
                return Err(SemaError::arity("__vm-prompt", "1", args.len()));
            }
            let entries = args[0]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;
            let mut messages = Vec::new();
            for entry in entries {
                if let Some(msg) = entry.as_message_rc() {
                    messages.push((*msg).clone());
                } else if let Some(pair) = entry.as_list() {
                    if pair.len() == 2 {
                        let role_str = pair[0]
                            .as_str()
                            .ok_or_else(|| SemaError::eval("prompt: expected role string"))?;
                        let role = match role_str {
                            "system" => Role::System,
                            "user" => Role::User,
                            "assistant" => Role::Assistant,
                            "tool" => Role::Tool,
                            other => {
                                return Err(SemaError::eval(format!(
                                    "prompt: unknown role '{other}'"
                                )))
                            }
                        };
                        let parts = pair[1]
                            .as_list()
                            .ok_or_else(|| SemaError::type_error("list", pair[1].type_name()))?;
                        let mut content = String::new();
                        for part in parts {
                            if let Some(s) = part.as_str() {
                                content.push_str(s);
                            } else {
                                content.push_str(&part.to_string());
                            }
                        }
                        messages.push(Message {
                            role,
                            content,
                            images: Vec::new(),
                        });
                    } else {
                        return Err(SemaError::eval(
                            "prompt: expected (role parts) pair or message value",
                        ));
                    }
                } else {
                    return Err(SemaError::eval(
                        "prompt: expected (role parts) pair or message value",
                    ));
                }
            }
            Ok(Value::prompt(Prompt { messages }))
        })),
    );

    // __vm-message: build Message directly from pre-evaluated parts
    env.set(
        intern("__vm-message"),
        Value::native_fn(NativeFn::simple("__vm-message", |args| {
            use sema_core::{Message, Role};
            if args.len() != 2 {
                return Err(SemaError::arity("__vm-message", "2", args.len()));
            }
            let role = if let Some(spur) = args[0].as_keyword_spur() {
                let s = resolve(spur);
                match s.as_str() {
                    "system" => Role::System,
                    "user" => Role::User,
                    "assistant" => Role::Assistant,
                    "tool" => Role::Tool,
                    other => {
                        return Err(SemaError::eval(format!("message: unknown role '{other}'")))
                    }
                }
            } else {
                return Err(SemaError::type_error("keyword", args[0].type_name()));
            };
            let parts = args[1]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[1].type_name()))?;
            let mut content = String::new();
            for part in parts {
                if let Some(s) = part.as_str() {
                    content.push_str(s);
                } else {
                    content.push_str(&part.to_string());
                }
            }
            Ok(Value::message(Message {
                role,
                content,
                images: Vec::new(),
            }))
        })),
    );

    // __vm-deftool: delegate to tree-walker
    // __vm-deftool: the VM has already evaluated description/parameters/handler
    // and passes them as values, so build the tool directly — no tree-walker
    // round-trip.
    let tool_env = env.clone();
    env.set(
        intern("__vm-deftool"),
        Value::native_fn(NativeFn::simple("__vm-deftool", move |args| {
            if args.len() != 4 {
                return Err(SemaError::arity("deftool", "4", args.len()));
            }
            let name = args[0]
                .as_symbol()
                .ok_or_else(|| SemaError::eval("deftool: name must be a symbol"))?;
            special_forms::register_tool(
                &name,
                args[1].clone(),
                args[2].clone(),
                args[3].clone(),
                &tool_env,
            )
        })),
    );

    // __vm-defagent: the VM has already evaluated the options map, so build the
    // agent directly — no tree-walker round-trip.
    let agent_env = env.clone();
    env.set(
        intern("__vm-defagent"),
        Value::native_fn(NativeFn::simple("__vm-defagent", move |args| {
            if args.len() != 2 {
                return Err(SemaError::arity("defagent", "2", args.len()));
            }
            let name = args[0]
                .as_symbol()
                .ok_or_else(|| SemaError::eval("defagent: name must be a symbol"))?;
            special_forms::register_agent(&name, args[1].clone(), &agent_env)
        })),
    );

    // __vm-destructure: strict destructure — errors on shape mismatch
    // (pattern value) -> map of bindings keyed by symbol
    env.set(
        intern("__vm-destructure"),
        Value::native_fn(NativeFn::simple("__vm-destructure", |args| {
            if args.len() != 2 {
                return Err(SemaError::arity("__vm-destructure", "2", args.len()));
            }
            let bindings = crate::destructure::destructure(&args[0], &args[1])?;
            let mut map = std::collections::BTreeMap::new();
            for (spur, val) in bindings {
                map.insert(Value::symbol_from_spur(spur), val);
            }
            Ok(Value::map(map))
        })),
    );

    // __vm-try-match: soft match — returns nil on no match, map of bindings on match
    // (pattern value) -> nil | map of bindings keyed by symbol
    env.set(
        intern("__vm-try-match"),
        Value::native_fn(NativeFn::simple("__vm-try-match", |args| {
            if args.len() != 2 {
                return Err(SemaError::arity("__vm-try-match", "2", args.len()));
            }
            match crate::destructure::try_match(&args[0], &args[1])? {
                Some(bindings) => {
                    let mut map = std::collections::BTreeMap::new();
                    for (spur, val) in bindings {
                        map.insert(Value::symbol_from_spur(spur), val);
                    }
                    Ok(Value::map(map))
                }
                None => Ok(Value::nil()),
            }
        })),
    );

    // __vm-make-multi: create a MultiMethod value
    env.set(
        intern("__vm-make-multi"),
        Value::native_fn(NativeFn::simple("__vm-make-multi", |args| {
            if args.len() != 2 {
                return Err(SemaError::arity("__vm-make-multi", "2", args.len()));
            }
            let name_spur = args[0]
                .as_symbol_spur()
                .ok_or_else(|| SemaError::eval("__vm-make-multi: expected symbol"))?;
            Ok(Value::multimethod(MultiMethod {
                name: name_spur,
                dispatch_fn: args[1].clone(),
                methods: RefCell::new(std::collections::BTreeMap::new()),
                default: RefCell::new(None),
            }))
        })),
    );

    // __vm-defmethod: add a method to an existing MultiMethod
    env.set(
        intern("__vm-defmethod"),
        Value::native_fn(NativeFn::simple("__vm-defmethod", |args| {
            if args.len() != 3 {
                return Err(SemaError::arity("__vm-defmethod", "3", args.len()));
            }
            let mm = args[0]
                .as_multimethod_rc()
                .ok_or_else(|| SemaError::eval("defmethod: first argument is not a multimethod"))?;
            let dispatch_val = &args[1];
            let handler = &args[2];
            if let Some(kw) = dispatch_val.as_keyword_spur() {
                if resolve(kw) == "default" {
                    *mm.default.borrow_mut() = Some(handler.clone());
                    return Ok(Value::nil());
                }
            }
            mm.methods
                .borrow_mut()
                .insert(dispatch_val.clone(), handler.clone());
            Ok(Value::nil())
        })),
    );
}
