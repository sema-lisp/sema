use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::{Rc, Weak};

use sema_core::{
    intern, resolve, Env, EvalContext, Macro, MultiMethod, NativeFn, SemaError, Spur, Thunk, Value,
    ValueView,
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

impl Drop for Interpreter {
    fn drop(&mut self) {
        // The thread-local scheduler holds an Rc clone of this interpreter's
        // global env (plus any leftover task VMs with their own clones);
        // release it so teardown actually frees the env. Guarded by ptr_eq
        // inside shutdown_scheduler — a different interpreter's scheduler on
        // the same thread is left alone.
        sema_vm::shutdown_scheduler(&self.global_env);
        // Skip the teardown collection while unwinding: a panic anywhere in
        // the collector would be a panic-in-destructor-during-cleanup, which
        // aborts the whole process instead of unwinding. Nothing is lost —
        // the candidates stay registered, so the next safe point on this
        // thread reclaims the env.
        if std::thread::panicking() {
            return;
        }
        // `self.ctx` outlives this Drop body (fields drop after it), and its
        // caches hold Values: module-cache export closures keep their module
        // envs — and via the parent chain the ENTIRE global env — externally
        // referenced, so with them held the teardown collect would free
        // nothing. Clear every ctx-held Value store first.
        self.ctx.module_cache.borrow_mut().clear();
        self.ctx.user_context.borrow_mut().clear();
        self.ctx.hidden_context.borrow_mut().clear();
        self.ctx.context_stacks.borrow_mut().clear();
        // Release this interpreter's own strong ref to the env BEFORE the
        // teardown collection: with it held, the env wrapper carries an
        // external count and trial deletion (correctly) keeps the whole env.
        // Once released, the only refs left are the Env⇄Closure cycle edges
        // from top-level `define`s, which the collector severs. No pins — the
        // dying env is exactly what must be traced. Anything still externally
        // held (e.g. a user-kept `global_env` clone) survives, and the
        // registry reclaims it at a later safe point once released.
        drop(std::mem::replace(&mut self.global_env, Rc::new(Env::new())));
        sema_core::gc_collect(&[], sema_core::GcTrigger::InterpreterDrop);
    }
}

impl Interpreter {
    pub fn new() -> Self {
        let env = Env::new();
        let ctx = EvalContext::new();
        // Register eval/call callbacks so stdlib can invoke the real evaluator
        sema_core::set_eval_callback(&ctx, eval_value_vm);
        sema_core::set_call_callback(&ctx, call_value);
        sema_core::set_call_owned_callback(&ctx, call_value_owned);
        // Register stdlib
        sema_stdlib::register_stdlib(&env, &sema_core::Sandbox::allow_all());
        // Register LLM builtins
        #[cfg(not(target_arch = "wasm32"))]
        {
            sema_llm::builtins::reset_runtime_state();
            sema_llm::builtins::register_llm_builtins(&env, &sema_core::Sandbox::allow_all());
        }
        let global_env = Rc::new(env);
        register_vm_delegates(&global_env);
        load_prelude(&ctx, &global_env);
        Interpreter { global_env, ctx }
    }

    pub fn new_with_sandbox(sandbox: &sema_core::Sandbox) -> Self {
        let env = Env::new();
        let ctx = EvalContext::new_with_sandbox(sandbox.clone());
        sema_core::set_eval_callback(&ctx, eval_value_vm);
        sema_core::set_call_callback(&ctx, call_value);
        sema_core::set_call_owned_callback(&ctx, call_value_owned);
        sema_stdlib::register_stdlib(&env, sandbox);
        #[cfg(not(target_arch = "wasm32"))]
        {
            sema_llm::builtins::reset_runtime_state();
            sema_llm::builtins::register_llm_builtins(&env, sandbox);
        }
        let global_env = Rc::new(env);
        register_vm_delegates(&global_env);
        load_prelude(&ctx, &global_env);
        Interpreter { global_env, ctx }
    }

    /// Evaluate a single expression on the VM. M6: the VM is the sole evaluator.
    ///
    /// NOTE (deliberate behavior change): all eval
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
        self.run_exprs_on_vm(std::slice::from_ref(expr), &self.global_env)
    }

    /// Parse and evaluate in the global environment so that `define` persists across calls.
    pub fn eval_str_in_global(&self, input: &str) -> EvalResult {
        let (exprs, spans) = sema_reader::read_many_with_spans(input)?;
        self.ctx.merge_span_table(spans);
        if exprs.is_empty() {
            return Ok(Value::nil());
        }
        self.run_exprs_on_vm(&exprs, &self.global_env)
    }

    /// Parse, compile to bytecode, and execute via the VM (global env, persists).
    pub fn eval_str_compiled(&self, input: &str) -> EvalResult {
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
        // Batch expansion: a top-level define in ANY form shadows a same-named
        // macro in EVERY form of this program (all forms expand before any
        // executes, so the env can't provide that shadowing naturally).
        let expanded = expand_for_vm_batch(&self.ctx, globals, exprs)?;
        let known_natives = collect_native_names(globals);
        let span_map = self.ctx.span_table.borrow().clone();
        let prog = sema_vm::compile_program_with_spans_and_natives(
            &expanded,
            &span_map,
            None,
            Some(known_natives),
        )?;
        let mut vm = sema_vm::VM::new(
            globals.clone(),
            prog.functions,
            &prog.native_table,
            prog.main_cache_slots,
        )?;
        sema_vm::init_scheduler(self.global_env.clone(), prog.native_table.clone());
        // Reset the loop-guard step counter so the limit (if any) is per top-level
        // eval, not cumulative across calls on a reused interpreter.
        self.ctx.eval_steps.set(0);
        let result = vm.execute(prog.closure, &self.ctx);
        // Cycle-collector safe point (CORE-2): a top-level form just finished
        // (REPL line, notebook cell, script form, embedded eval), so no VM
        // frames or env borrows are live. Pins skip descent into this
        // interpreter's global namespace; the scheduler's globals are the same
        // env (init_scheduler above), so the one chain covers both.
        if sema_core::gc_should_collect() {
            sema_core::gc_threshold_collect(
                &sema_core::gc_env_chain_pins(&self.global_env),
                sema_core::GcTrigger::EvalReturn,
            );
        }
        result
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

    /// Pre-process a top-level expression for VM compilation: register any
    /// `defmacro` forms, then expand macro calls in all other forms.
    pub fn expand_for_vm(&self, expr: &Value) -> EvalResult {
        expand_for_vm_in(&self.ctx, &self.global_env, expr)
    }

    /// Expand a multi-form program with cross-form define shadowing — use this
    /// (not per-form `expand_for_vm`) whenever all forms expand before any runs.
    pub fn expand_for_vm_batch(&self, exprs: &[Value]) -> Result<Vec<Value>, SemaError> {
        expand_for_vm_batch(&self.ctx, &self.global_env, exprs)
    }
}

/// Lexical names that shadow macros during expansion (a linked stack of
/// frames, innermost first). Macro expansion is name-based and runs before
/// scope resolution; without this, a user binding named after a prelude macro
/// (`step`, `phase`, ...) is rewritten as a macro call — in a define-sugar
/// head that is a hard compile error, and for `phase`-shaped macros it
/// silently clobbers the runtime binding the template calls.
struct Shadow<'a> {
    names: HashSet<Spur>,
    parent: Option<&'a Shadow<'a>>,
}

impl<'a> Shadow<'a> {
    fn child(&'a self, names: HashSet<Spur>) -> Shadow<'a> {
        Shadow {
            names,
            parent: Some(self),
        }
    }

    fn contains(&self, s: Spur) -> bool {
        self.names.contains(&s) || self.parent.is_some_and(|p| p.contains(s))
    }
}

/// Collect every symbol in a binding pattern (a param list, a let binding
/// name, a match/destructure pattern). Deliberately conservative: any symbol
/// anywhere in the pattern counts as bound. Over-collecting only suppresses
/// macro expansion where a same-named binding plausibly exists — the safe
/// direction.
fn collect_pattern_symbols(pattern: &Value, out: &mut HashSet<Spur>) {
    if let Some(s) = pattern.as_symbol_spur() {
        out.insert(s);
        return;
    }
    if let Some(items) = pattern.as_list() {
        for item in items {
            collect_pattern_symbols(item, out);
        }
        return;
    }
    match pattern.view() {
        ValueView::Vector(items) => {
            for item in items.iter() {
                collect_pattern_symbols(item, out);
            }
        }
        ValueView::Map(map) => {
            for (k, v) in map.iter() {
                collect_pattern_symbols(k, out);
                collect_pattern_symbols(v, out);
            }
        }
        _ => {}
    }
}

/// Names a form defines at its sequence level (top level or a body), for
/// letrec*-style shadowing: `define` (sugar + plain), `define-values`,
/// `defmulti`, `deftool`, `defagent`, and `define-record-type` (constructor,
/// predicate, and accessors included). Recurses into `begin`/`progn`.
fn collect_defined_names(expr: &Value, out: &mut HashSet<Spur>) {
    let Some(items) = expr.as_list() else { return };
    let Some(head) = items.first().and_then(|v| v.as_symbol_spur()) else {
        return;
    };
    match resolve(head).as_str() {
        "begin" | "progn" => {
            for item in &items[1..] {
                collect_defined_names(item, out);
            }
        }
        "define" | "defmulti" | "deftool" | "defagent" => {
            if let Some(target) = items.get(1) {
                if let Some(s) = target.as_symbol_spur() {
                    out.insert(s);
                } else if let Some(sugar) = target.as_list() {
                    // Sugar head: (define (name . params) ...) defines `name`.
                    if let Some(s) = sugar.first().and_then(|v| v.as_symbol_spur()) {
                        out.insert(s);
                    }
                }
            }
        }
        "define-values" => {
            if let Some(formals) = items.get(1) {
                collect_pattern_symbols(formals, out);
            }
        }
        "define-record-type" => {
            // (define-record-type Name (ctor field...) pred (field accessor [setter])...)
            for part in &items[1..] {
                collect_pattern_symbols(part, out);
            }
        }
        _ => {}
    }
}

/// Expand the forms of a body sequence: names defined anywhere in the body
/// shadow macros throughout it (letrec* semantics, matching the resolver).
fn expand_body(
    ctx: &EvalContext,
    env: &Env,
    body: &[Value],
    shadow: &Shadow,
) -> Result<Vec<Value>, SemaError> {
    let mut defined = HashSet::new();
    for form in body {
        collect_defined_names(form, &mut defined);
    }
    let inner = shadow.child(defined);
    body.iter()
        .map(|form| expand_macros_in(ctx, env, form, &inner))
        .collect()
}

/// Rebuild a list form only if any element changed, preserving Rc pointer
/// identity otherwise (span lookups are keyed by pointer).
fn rebuilt_list(original: &Value, items: &[Value], expanded: Vec<Value>) -> Value {
    let changed = expanded
        .iter()
        .zip(items.iter())
        .any(|(a, b)| a.raw_bits() != b.raw_bits())
        || expanded.len() != items.len();
    if changed {
        Value::list(expanded)
    } else {
        original.clone()
    }
}

/// Pre-process a top-level expression for VM compilation, expanding macro calls
/// and eagerly registering `defmacro` forms — against `env` rather than a fixed
/// global env. For top-level code `env` is the global env (unchanged behavior);
/// for a `load`ed module body it is the same shared global env, so a `defmacro`
/// registers where `expand_macros_in` looks it up and inherited macros still
/// resolve via the parent chain.
///
/// A form's own `define`s shadow same-named macros inside it. For a multi-form
/// program use [`expand_for_vm_batch`], which lets a top-level
/// `(define step ...)` shadow the macro in sibling forms too.
pub fn expand_for_vm_in(ctx: &EvalContext, env: &Env, expr: &Value) -> EvalResult {
    let mut defined = HashSet::new();
    collect_defined_names(expr, &mut defined);
    let shadow = Shadow {
        names: defined,
        parent: None,
    };
    expand_top_form(ctx, env, expr, &shadow)
}

/// Expand a whole multi-form program: names defined by ANY top-level form
/// shadow same-named macros in EVERY form (mirroring the compiler's
/// redefined-globals rule for intrinsics), so `(define (step n) n) (step 3)`
/// calls the user's function rather than expanding the prelude macro.
pub fn expand_for_vm_batch(
    ctx: &EvalContext,
    env: &Env,
    exprs: &[Value],
) -> Result<Vec<Value>, SemaError> {
    let mut defined = HashSet::new();
    for expr in exprs {
        collect_defined_names(expr, &mut defined);
    }
    let shadow = Shadow {
        names: defined,
        parent: None,
    };
    exprs
        .iter()
        .map(|expr| expand_top_form(ctx, env, expr, &shadow))
        .collect()
}

fn expand_top_form(ctx: &EvalContext, env: &Env, expr: &Value, shadow: &Shadow) -> EvalResult {
    if let Some(items) = expr.as_list() {
        if let Some(s) = items.first().and_then(|v| v.as_symbol_spur()) {
            let name = resolve(s);
            if name == "defmacro" {
                // Register the macro directly (pure destructure) — the VM macro
                // path is direct.
                register_defmacro(items, env)?;
                return Ok(Value::nil());
            }
            if name == "define-syntax" {
                // Register the R7RS syntax-rules transformer directly (pure
                // destructure), mirroring the `defmacro` branch.
                register_define_syntax(items, env)?;
                return Ok(Value::nil());
            }
            if name == "begin" || name == "progn" {
                let mut new_items = vec![Value::symbol_from_spur(s)];
                let mut changed = false;
                for item in &items[1..] {
                    let expanded = expand_top_form(ctx, env, item, shadow)?;
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
    expand_macros_in(ctx, env, expr, shadow)
}

/// Recursively expand macro calls, resolving macros via `env` (walking the
/// parent chain). Scope-aware: binding positions (define-sugar heads, params,
/// let names, match patterns) never expand, and a head symbol that a lexical
/// binding shadows is treated as an ordinary call. Preserves Rc pointer
/// identity when no expansion occurs so span lookups (keyed by Rc pointer)
/// remain valid.
fn expand_macros_in(ctx: &EvalContext, env: &Env, expr: &Value, shadow: &Shadow) -> EvalResult {
    if let Some(items) = expr.as_list() {
        if !items.is_empty() {
            if let Some(s) = items.first().and_then(|v| v.as_symbol_spur()) {
                let name = resolve(s);
                if name == "quote" {
                    return Ok(expr.clone());
                }
                // Binding forms expand structurally so their bound names
                // shadow macros in exactly the scopes the resolver gives them.
                match name.as_str() {
                    "define" => return expand_define_form(ctx, env, expr, items, shadow),
                    "fn" | "lambda" => return expand_lambda_form(ctx, env, expr, items, shadow),
                    "let" | "let*" | "letrec" | "let-values" | "let*-values" => {
                        return expand_let_form(ctx, env, expr, items, shadow, &name)
                    }
                    "do" => return expand_do_form(ctx, env, expr, items, shadow),
                    "try" => return expand_try_form(ctx, env, expr, items, shadow),
                    "match" | "match*" => return expand_match_form(ctx, env, expr, items, shadow),
                    "define-values" => {
                        // Formals are a binding position; only the value expands.
                        let mut expanded: Vec<Value> = items[..items.len().min(2)].to_vec();
                        for item in items.iter().skip(2) {
                            expanded.push(expand_macros_in(ctx, env, item, shadow)?);
                        }
                        return Ok(rebuilt_list(expr, items, expanded));
                    }
                    _ => {}
                }
                if !shadow.contains(s) {
                    if let Some(mac_val) = env.get(s) {
                        if let Some(mac) = mac_val.as_macro_rc() {
                            if mac.syntax_rules.is_some() {
                                // R7RS syntax-rules: pattern-match + template expand.
                                let expanded = crate::syntax_rules::expand(&mac, &items[1..], env)?;
                                return expand_macros_in(ctx, env, &expanded, shadow);
                            }
                            // VM-native expansion: apply the transformer on the VM.
                            let expanded = apply_macro_vm(ctx, &mac, &items[1..], env)?;
                            return expand_macros_in(ctx, env, &expanded, shadow);
                        }
                    }
                }
            }
            let expanded: Vec<Value> = items
                .iter()
                .map(|v| expand_macros_in(ctx, env, v, shadow))
                .collect::<Result<_, _>>()?;
            return Ok(rebuilt_list(expr, items, expanded));
        }
    }

    match expr.view() {
        ValueView::Vector(items) => {
            let expanded: Vec<Value> = items
                .iter()
                .map(|v| expand_macros_in(ctx, env, v, shadow))
                .collect::<Result<_, _>>()?;
            let changed = expanded
                .iter()
                .zip(items.iter())
                .any(|(a, b)| a.raw_bits() != b.raw_bits());
            if changed {
                Ok(Value::vector(expanded))
            } else {
                Ok(expr.clone())
            }
        }
        ValueView::Map(map) => {
            let mut changed = false;
            let mut expanded = BTreeMap::new();
            for (key, value) in map.iter() {
                let expanded_key = expand_macros_in(ctx, env, key, shadow)?;
                let expanded_value = expand_macros_in(ctx, env, value, shadow)?;
                changed |= expanded_key.raw_bits() != key.raw_bits()
                    || expanded_value.raw_bits() != value.raw_bits();
                expanded.insert(expanded_key, expanded_value);
            }
            if changed {
                Ok(Value::map(expanded))
            } else {
                Ok(expr.clone())
            }
        }
        _ => Ok(expr.clone()),
    }
}

/// `(define name expr)` / `(define (name . params) body...)`: the head is a
/// binding position (never expanded); the defined name and any params shadow
/// macros in the value/body.
fn expand_define_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
) -> EvalResult {
    let Some(target) = items.get(1) else {
        return Ok(expr.clone());
    };
    let mut bound = HashSet::new();
    collect_pattern_symbols(target, &mut bound);
    let inner = shadow.child(bound);
    let mut expanded: Vec<Value> = items[..2.min(items.len())].to_vec();
    expanded.extend(expand_body(ctx, env, &items[2..], &inner)?);
    Ok(rebuilt_list(expr, items, expanded))
}

/// `(fn params body...)`: params are a binding position and shadow the body.
fn expand_lambda_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
) -> EvalResult {
    let Some(params) = items.get(1) else {
        return Ok(expr.clone());
    };
    let mut bound = HashSet::new();
    collect_pattern_symbols(params, &mut bound);
    let inner = shadow.child(bound);
    let mut expanded: Vec<Value> = items[..2.min(items.len())].to_vec();
    expanded.extend(expand_body(ctx, env, &items[2..], &inner)?);
    Ok(rebuilt_list(expr, items, expanded))
}

/// The `let` family, named `let` included. Init scoping follows the form:
/// `let`/`let-values` inits see the outer scope, the starred forms see the
/// bindings accumulated so far, `letrec` inits see everything.
fn expand_let_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
    form: &str,
) -> EvalResult {
    // Named let: (let name ((v init)...) body...)
    let named = form == "let" && items.get(1).is_some_and(|v| v.as_symbol_spur().is_some());
    let bindings_idx = if named { 2 } else { 1 };
    let Some(bindings_form) = items.get(bindings_idx) else {
        return Ok(expr.clone());
    };
    let pairs: Vec<Value> = if let Some(l) = bindings_form.as_list() {
        l.to_vec()
    } else if let ValueView::Vector(v) = bindings_form.view() {
        v.to_vec()
    } else {
        // Malformed; let the lowering report it. Expand generically.
        let expanded: Vec<Value> = items
            .iter()
            .map(|v| expand_macros_in(ctx, env, v, shadow))
            .collect::<Result<_, _>>()?;
        return Ok(rebuilt_list(expr, items, expanded));
    };

    let mut bound = HashSet::new();
    if named {
        collect_pattern_symbols(&items[1], &mut bound);
    }
    if form == "letrec" {
        for pair in &pairs {
            if let Some(p) = pair.as_list().and_then(|p| p.first().cloned()) {
                collect_pattern_symbols(&p, &mut bound);
            }
        }
    }

    let sequential = form == "let*" || form == "let*-values";
    let mut new_pairs = Vec::with_capacity(pairs.len());
    for pair in &pairs {
        let Some(pair_items) = pair.as_list() else {
            new_pairs.push(pair.clone());
            continue;
        };
        let init_scope = shadow.child(bound.clone());
        let mut new_pair: Vec<Value> = Vec::with_capacity(pair_items.len());
        for (i, part) in pair_items.iter().enumerate() {
            if i == 0 {
                // The binding pattern itself never expands.
                new_pair.push(part.clone());
            } else {
                new_pair.push(expand_macros_in(ctx, env, part, &init_scope)?);
            }
        }
        if sequential || form == "letrec" {
            if let Some(p) = pair_items.first() {
                collect_pattern_symbols(p, &mut bound);
            }
            new_pairs.push(rebuilt_list(pair, pair_items, new_pair));
        } else {
            new_pairs.push(rebuilt_list(pair, pair_items, new_pair));
        }
    }
    if !sequential && form != "letrec" {
        // Plain let / let-values: all names bind only in the body.
        for pair in &pairs {
            if let Some(p) = pair.as_list().and_then(|p| p.first().cloned()) {
                collect_pattern_symbols(&p, &mut bound);
            }
        }
    }

    let body_scope = shadow.child(bound);
    let mut expanded: Vec<Value> = items[..bindings_idx].to_vec();
    expanded.push(Value::list(new_pairs));
    expanded.extend(expand_body(
        ctx,
        env,
        &items[bindings_idx + 1..],
        &body_scope,
    )?);
    Ok(rebuilt_list(expr, items, expanded))
}

/// Scheme `do`: `(do ((var init step)...) (test result...) body...)` — vars
/// bind in steps, the test/result, and the body; inits see the outer scope.
fn expand_do_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
) -> EvalResult {
    let Some(specs) = items.get(1).and_then(|v| v.as_list().map(|l| l.to_vec())) else {
        let expanded: Vec<Value> = items
            .iter()
            .map(|v| expand_macros_in(ctx, env, v, shadow))
            .collect::<Result<_, _>>()?;
        return Ok(rebuilt_list(expr, items, expanded));
    };
    let mut bound = HashSet::new();
    for spec in &specs {
        if let Some(p) = spec.as_list().and_then(|p| p.first().cloned()) {
            collect_pattern_symbols(&p, &mut bound);
        }
    }
    let inner = shadow.child(bound);
    let mut new_specs = Vec::with_capacity(specs.len());
    for spec in &specs {
        let Some(spec_items) = spec.as_list() else {
            new_specs.push(spec.clone());
            continue;
        };
        let mut new_spec = Vec::with_capacity(spec_items.len());
        for (i, part) in spec_items.iter().enumerate() {
            match i {
                0 => new_spec.push(part.clone()),
                1 => new_spec.push(expand_macros_in(ctx, env, part, shadow)?),
                _ => new_spec.push(expand_macros_in(ctx, env, part, &inner)?),
            }
        }
        new_specs.push(rebuilt_list(spec, spec_items, new_spec));
    }
    let mut expanded: Vec<Value> = vec![items[0].clone(), Value::list(new_specs)];
    for item in items.iter().skip(2) {
        expanded.push(expand_macros_in(ctx, env, item, &inner)?);
    }
    Ok(rebuilt_list(expr, items, expanded))
}

/// `try`: catch clauses bind their error variable over the handler body.
fn expand_try_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
) -> EvalResult {
    let mut expanded: Vec<Value> = vec![items[0].clone()];
    for item in items.iter().skip(1) {
        let is_catch = item
            .as_list()
            .and_then(|l| l.first().and_then(|h| h.as_symbol_spur()))
            .is_some_and(|h| resolve(h) == "catch");
        if is_catch {
            let clause = item.as_list().unwrap();
            let mut bound = HashSet::new();
            if let Some(var) = clause.get(1) {
                collect_pattern_symbols(var, &mut bound);
            }
            let inner = shadow.child(bound);
            let mut new_clause: Vec<Value> = clause[..2.min(clause.len())].to_vec();
            for part in clause.iter().skip(2) {
                new_clause.push(expand_macros_in(ctx, env, part, &inner)?);
            }
            expanded.push(rebuilt_list(item, clause, new_clause));
        } else {
            expanded.push(expand_macros_in(ctx, env, item, shadow)?);
        }
    }
    Ok(rebuilt_list(expr, items, expanded))
}

/// `match`/`match*`: each clause's pattern is a binding position (never
/// expanded) whose symbols shadow the clause's guard and body.
fn expand_match_form(
    ctx: &EvalContext,
    env: &Env,
    expr: &Value,
    items: &[Value],
    shadow: &Shadow,
) -> EvalResult {
    let mut expanded: Vec<Value> = vec![items[0].clone()];
    if let Some(scrutinee) = items.get(1) {
        expanded.push(expand_macros_in(ctx, env, scrutinee, shadow)?);
    }
    for clause in items.iter().skip(2) {
        let parts: Option<Vec<Value>> = if let Some(l) = clause.as_list() {
            Some(l.to_vec())
        } else if let ValueView::Vector(v) = clause.view() {
            Some(v.to_vec())
        } else {
            None
        };
        let Some(parts) = parts else {
            expanded.push(expand_macros_in(ctx, env, clause, shadow)?);
            continue;
        };
        if parts.is_empty() {
            expanded.push(clause.clone());
            continue;
        }
        let mut bound = HashSet::new();
        collect_pattern_symbols(&parts[0], &mut bound);
        let inner = shadow.child(bound);
        let mut new_parts = vec![parts[0].clone()];
        for part in parts.iter().skip(1) {
            new_parts.push(expand_macros_in(ctx, env, part, &inner)?);
        }
        let changed = new_parts
            .iter()
            .zip(parts.iter())
            .any(|(a, b)| a.raw_bits() != b.raw_bits());
        if !changed {
            expanded.push(clause.clone());
        } else if clause.as_list().is_some() {
            expanded.push(Value::list(new_parts));
        } else {
            expanded.push(Value::vector(new_parts));
        }
    }
    Ok(rebuilt_list(expr, items, expanded))
}

/// Run deserialized bytecode/// Run deserialized bytecode (a `.semac` payload) on a fresh VM rooted at
/// `globals`. Used to `load`/`import` precompiled bytecode modules (e.g.
/// embedded in a standalone-executable or web-archive VFS) the same way
/// `eval_module_body_vm` runs source modules. Does NOT (re)initialize the
/// async scheduler — callers nest this inside an already-running program and
/// reuse the scheduler installed by the top-level VM driver.
pub fn execute_compile_result(
    ctx: &EvalContext,
    globals: Rc<Env>,
    result: sema_vm::CompileResult,
) -> Result<Value, SemaError> {
    let functions: Vec<Rc<sema_vm::Function>> = result.functions.into_iter().map(Rc::new).collect();
    let main_cache_slots = result.chunk.n_global_cache_slots;
    let closure = Rc::new(sema_vm::Closure {
        func: Rc::new(sema_vm::Function {
            name: None,
            chunk: result.chunk,
            upvalue_descs: Vec::new(),
            upvalue_names: Vec::new(),
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: Vec::new(),
            local_scopes: Vec::new(),
            source_file: None,
            cache_offset: 0,
        }),
        upvalues: Vec::new(),
        globals: None,
        functions: None,
    });

    let mut vm = sema_vm::VM::new(globals, functions, &[], main_cache_slots)?;
    vm.execute(closure, ctx)
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
    // Each per-form VM ran on `Rc::new(env.clone())`; the clone shares both
    // `env`'s bindings map and its version cell (`Env::version` is `Rc`-held),
    // so any global the body (re)defined or `set!`d already bumped the version
    // the calling VM's inline cache is keyed on — no explicit re-bump needed.
    Ok(result)
}

/// VM-native evaluation for callback consumers (e.g. sema-llm tool handlers):
/// macro-expand, compile, and run `expr` on a fresh bytecode VM rooted at `env`.
/// This is used to keep the
/// eval-callback path on the VM. Each call builds a
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
        ValueView::Lambda(_) => {
            // Raw `Lambda` values never occur on the VM path (user lambdas are
            // NativeFn-wrapped VM closures).
            Err(SemaError::eval(
                "internal: raw lambda value reached call_value (VM closures are native-fn-wrapped)"
                    .to_string(),
            ))
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

/// Like [`call_value`], but the caller passes an args buffer it OWNS and will
/// not reuse: a VM-closure callee moves the values into its frame slots (the
/// buffer is left holding nils), so a uniquely-owned accumulator stays
/// uniquely owned across the callback boundary — the enabler for the stdlib's
/// `strong_count == 1` in-place fast paths inside fold callbacks. Every other
/// callable falls back to the borrowed protocol (args intact).
pub fn call_value_owned(ctx: &EvalContext, func: &Value, args: &mut [Value]) -> EvalResult {
    if let Some(result) = sema_vm::call_closure_owned(func, ctx, args) {
        return result;
    }
    call_value(ctx, func, args)
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
/// Apply a macro by evaluating its body on the **bytecode VM**.
///
/// The macro's
/// (unevaluated) arguments are bound — together with a possible rest list — as
/// *globals* in a transient child env of `caller_env`; the transformer body is
/// then compiled fresh per call site (so auto-gensym stays hygienic — a cached
/// transformer would reuse the same gensym across call sites) and run on a VM
/// rooted at that env. Rooting at `caller_env` lets transformer bodies call
/// global helpers and reference module-level bindings, and binding params as
/// globals lets the compiled body resolve them via `GetGlobal`.
///
/// Used by the VM macro pre-expansion path (`expand_macros_in`) and
/// `__vm-macroexpand`.
pub fn apply_macro_vm(
    ctx: &EvalContext,
    mac: &sema_core::Macro,
    args: &[Value],
    caller_env: &Env,
) -> Result<Value, SemaError> {
    let env = Rc::new(Env::with_parent(Rc::new(caller_env.clone())));

    // Bind parameters to unevaluated forms.
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
    // unquote / unquote-splicing directly.
    let mut result = Value::nil();
    for expr in &mac.body {
        let prog = sema_vm::compile_program(std::slice::from_ref(expr), None)?;
        let mut vm = sema_vm::VM::new(env.clone(), prog.functions, &[], prog.main_cache_slots)?;
        result = vm.execute(prog.closure, ctx)?;
    }
    Ok(result)
}

/// Register a `defmacro` form's macro in `env` — a
/// pure destructure mirroring `special_forms::eval_defmacro`. Used by the VM
/// pre-expansion path so registering a macro is direct.
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
            syntax_rules: None,
        }),
    );
    Ok(())
}

/// Register a `define-syntax` form's R7RS `syntax-rules` transformer in `env`
/// (pure destructure) — the syntax-rules counterpart of [`register_defmacro`].
/// `items[0]` is the `define-syntax` symbol; the rest are name + transformer.
fn register_define_syntax(items: &[Value], env: &Env) -> Result<(), SemaError> {
    let args = &items[1..];
    if args.len() != 2 {
        return Err(SemaError::eval(
            "define-syntax: expected (define-syntax name (syntax-rules ...))",
        ));
    }
    let name_spur = args[0]
        .as_symbol_spur()
        .ok_or_else(|| SemaError::eval("define-syntax: name must be a symbol"))?;
    let sr = parse_syntax_rules(&args[1])?;
    env.set(
        name_spur,
        Value::macro_val(Macro {
            params: Vec::new(),
            rest_param: None,
            body: Vec::new(),
            name: name_spur,
            syntax_rules: Some(Rc::new(sr)),
        }),
    );
    Ok(())
}

/// Parse a `(syntax-rules (literals...) (pattern template)...)` transformer form
/// — with an optional custom-ellipsis symbol before the literals list — into a
/// [`sema_core::SyntaxRules`].
fn parse_syntax_rules(form: &Value) -> Result<sema_core::SyntaxRules, SemaError> {
    let elems = form.as_list().ok_or_else(|| {
        SemaError::eval("define-syntax: transformer must be a (syntax-rules ...) form")
    })?;
    let head_ok = elems
        .first()
        .and_then(|v| v.as_symbol_spur())
        .is_some_and(|s| resolve(s) == "syntax-rules");
    if !head_ok {
        return Err(SemaError::eval(
            "define-syntax: transformer must be a (syntax-rules ...) form",
        ));
    }
    if elems.len() < 2 {
        return Err(SemaError::eval(
            "syntax-rules: malformed — expected (syntax-rules (literals...) rules...)",
        ));
    }
    // Optional custom ellipsis: a symbol in the slot where the literals list is
    // otherwise expected.
    let mut idx = 1;
    let ellipsis = if elems[idx].as_symbol_spur().is_some() {
        let e = elems[idx].as_symbol_spur().unwrap();
        idx += 1;
        e
    } else {
        intern("...")
    };
    let literals_val = elems
        .get(idx)
        .ok_or_else(|| SemaError::eval("syntax-rules: malformed — missing literals list"))?;
    let literals_list = literals_val
        .as_list()
        .ok_or_else(|| SemaError::eval("syntax-rules: literals must be a list"))?;
    let literals: Vec<Spur> = literals_list
        .iter()
        .map(|v| {
            v.as_symbol_spur()
                .ok_or_else(|| SemaError::eval("syntax-rules: each literal must be a symbol"))
        })
        .collect::<Result<_, _>>()?;
    idx += 1;
    let mut rules = Vec::new();
    for rule in &elems[idx..] {
        let rl = rule
            .as_list()
            .ok_or_else(|| SemaError::eval("syntax-rules: each rule must be (pattern template)"))?;
        if rl.len() < 2 {
            return Err(SemaError::eval(
                "syntax-rules: each rule must be (pattern template)",
            ));
        }
        let pattern = rl[0].clone();
        // R7RS rules have a single template; tolerate multiple by wrapping them
        // in an implicit `begin`.
        let template = if rl.len() == 2 {
            rl[1].clone()
        } else {
            let mut begin = vec![Value::symbol("begin")];
            begin.extend(rl[1..].iter().cloned());
            Value::list(begin)
        };
        rules.push((pattern, template));
    }
    Ok(sema_core::SyntaxRules {
        literals,
        ellipsis,
        rules,
    })
}

/// Register `__vm-*` native functions that the bytecode VM calls back into
/// the evaluator for forms that cannot be fully compiled.
/// Load built-in macros (threading, when-let, if-let) into the global environment.
pub fn load_prelude(ctx: &EvalContext, env: &Rc<Env>) {
    let exprs = sema_reader::read_many(crate::prelude::PRELUDE)
        .unwrap_or_else(|e| panic!("internal: prelude failed to parse: {e}"));
    // The prelude is mostly `defmacro` forms (which expand to nil, registering the
    // macro as a side effect) plus a few `define` forms (the async agent-loop driver).
    // Register/expand via the VM-native path; a `define`
    // expands to a non-nil form, which we compile + run on
    // the VM (rooted at the global env) so its top-level binding persists.
    for expr in &exprs {
        let expanded = expand_for_vm_in(ctx, env, expr)
            .unwrap_or_else(|e| panic!("internal: prelude failed to load: {e}"));
        if expanded.is_nil() {
            continue;
        }
        let prog = sema_vm::compile_program(std::slice::from_ref(&expanded), None)
            .unwrap_or_else(|e| panic!("internal: prelude failed to compile: {e}"));
        let mut vm = sema_vm::VM::new(env.clone(), prog.functions, &[], prog.main_cache_slots)
            .unwrap_or_else(|e| panic!("internal: prelude VM init failed: {e}"));
        vm.execute(prog.closure, ctx)
            .unwrap_or_else(|e| panic!("internal: prelude failed to evaluate: {e}"));
    }
}

/// Upgrade a delegate's weak env capture. Delegates are only callable through
/// the env that owns them (compiled code resolves `__vm-*` as globals in that
/// env), so a failed upgrade is unreachable in practice — the error is defense,
/// not semantics.
fn upgrade_delegate_env(weak: &Weak<Env>) -> Result<Rc<Env>, SemaError> {
    weak.upgrade()
        .ok_or_else(|| SemaError::eval("evaluator environment has been torn down"))
}

/// Register the `__vm-*` delegate natives into `env`.
///
/// Invariant I2 (CORE-2): each delegate's boxed closure captures the env it is
/// registered into WEAKLY (`Weak<Env>`), never strongly — a strong capture
/// would form an uncollectable `Env → NativeFn → Box<dyn Fn> → Env` cycle that
/// pins the entire environment past Interpreter teardown.
pub fn register_vm_delegates(env: &Rc<Env>) {
    // __vm-eval: macro-expand, compile, and run the expression on the bytecode
    // VM (rooted at the global env so top-level `define`s persist). The runtime
    // `(eval ...)` meta path is thus VM-native.
    let eval_env = Rc::downgrade(env);
    env.set(
        intern("__vm-eval"),
        Value::native_fn(NativeFn::with_ctx("__vm-eval", move |ctx, args| {
            if args.len() != 1 {
                return Err(SemaError::arity("eval", "1", args.len()));
            }
            let eval_env = upgrade_delegate_env(&eval_env)?;
            let expanded = expand_for_vm_in(ctx, &eval_env, &args[0])?;
            // A form that expands to nothing (e.g. a `defmacro`) yields nil.
            if expanded.is_nil() {
                return Ok(Value::nil());
            }
            let prog = sema_vm::compile_program(std::slice::from_ref(&expanded), None)?;
            let mut vm = sema_vm::VM::new(eval_env, prog.functions, &[], prog.main_cache_slots)?;
            vm.execute(prog.closure, ctx)
        })),
    );

    // __vm-module-exports: register a `(module name (export ...) ...)` form's
    // declared export list with the active module-load scope, so `import`
    // restricts the copied bindings to exactly those names. Without this the VM
    // exported every top-level binding (private helpers leaked). Mirrors the
    // module loader's `set_module_exports` call in eval_module.
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

    // __vm-load: call the load driver (special_forms::eval_load) directly.
    // The driver handles VFS
    // resolution, file path push/pop, caching, and runs the loaded body on the
    // VM (M4). The path arrives already evaluated from the VM.
    let load_env = Rc::downgrade(env);
    env.set(
        intern("__vm-load"),
        Value::native_fn(NativeFn::with_ctx("__vm-load", move |ctx, args| {
            if args.len() != 1 {
                return Err(SemaError::arity("load", "1", args.len()));
            }
            // Target the *currently executing* VM's env (the module being run),
            // falling back to the global env at top level, so a nested `load`
            // adds definitions to the right module env — not always the globals.
            let target = match sema_vm::current_vm_globals() {
                Some(t) => t,
                None => upgrade_delegate_env(&load_env)?,
            };
            match special_forms::eval_load(std::slice::from_ref(&args[0]), &target, ctx)? {
                Trampoline::Value(v) => Ok(v),
                Trampoline::Eval(..) => Ok(Value::nil()),
            }
        })),
    );

    // __vm-import: call the import driver (special_forms::eval_import) directly.
    // Under the VM backend the
    // driver compiles and runs the module body on the VM (M4). The path and
    // selective-import symbols arrive already evaluated from the VM.
    let import_env = Rc::downgrade(env);
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
            let target = match sema_vm::current_vm_globals() {
                Some(t) => t,
                None => upgrade_delegate_env(&import_env)?,
            };
            match special_forms::eval_import(&imp_args, &target, ctx)? {
                Trampoline::Value(v) => Ok(v),
                Trampoline::Eval(..) => Ok(Value::nil()),
            }
        })),
    );

    // __vm-defmacro: register a macro in the environment
    let macro_env = Rc::downgrade(env);
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
            let macro_env = upgrade_delegate_env(&macro_env)?;
            macro_env.set(
                name,
                Value::macro_val(Macro {
                    params,
                    rest_param,
                    body,
                    name,
                    syntax_rules: None,
                }),
            );
            Ok(Value::nil())
        })),
    );

    // __vm-defmacro-form: register a complete `(defmacro ...)` form directly
    // (pure destructure). Used for defmacro that
    // reaches compilation (e.g. non-top-level) rather than expand-time
    // registration.
    let dmf_env = Rc::downgrade(env);
    env.set(
        intern("__vm-defmacro-form"),
        Value::native_fn(NativeFn::simple("__vm-defmacro-form", move |args| {
            if args.len() != 1 {
                return Err(SemaError::arity("defmacro-form", "1", args.len()));
            }
            let items = args[0]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;
            let dmf_env = upgrade_delegate_env(&dmf_env)?;
            register_defmacro(items, &dmf_env)?;
            Ok(Value::nil())
        })),
    );

    // __vm-define-syntax: register a complete `(define-syntax ...)` form directly
    // (pure destructure). Used when a define-syntax reaches compilation (e.g.
    // non-top-level) rather than expand-time registration.
    let dsf_env = Rc::downgrade(env);
    env.set(
        intern("__vm-define-syntax"),
        Value::native_fn(NativeFn::simple("__vm-define-syntax", move |args| {
            if args.len() != 1 {
                return Err(SemaError::arity("define-syntax", "1", args.len()));
            }
            let items = args[0]
                .as_list()
                .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;
            let dsf_env = upgrade_delegate_env(&dsf_env)?;
            register_define_syntax(items, &dsf_env)?;
            Ok(Value::nil())
        })),
    );

    // __vm-define-record-type: delegate to the evaluator
    let drt_env = Rc::downgrade(env);
    env.set(
        intern("__vm-define-record-type"),
        Value::native_fn(NativeFn::simple("__vm-define-record-type", move |args| {
            if args.len() != 5 {
                return Err(SemaError::arity("define-record-type", "5", args.len()));
            }
            // Build the `(define-record-type ...)` argument list (without the head
            // symbol) and register the type directly via the pure destructure.
            // eval_define_record_type only sets native
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
            let drt_env = upgrade_delegate_env(&drt_env)?;
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
    let force_env = Rc::downgrade(env);
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
                    // Non-callable thunk body (a raw expr) — evaluate on the VM.
                    let force_env = upgrade_delegate_env(&force_env)?;
                    eval_value_vm(ctx, &thunk.body, &force_env)?
                };
                *thunk.forced.borrow_mut() = Some(val.clone());
                Ok(val)
            } else {
                Err(SemaError::type_error("thunk", args[0].type_name())
                    .with_hint("force: argument must be a (delay ...) or promise — non-promise values are an error"))
            }
        })),
    );

    // __vm-macroexpand: expand a macro form
    let me_env = Rc::downgrade(env);
    env.set(
        intern("__vm-macroexpand"),
        Value::native_fn(NativeFn::with_ctx("__vm-macroexpand", move |ctx, args| {
            if args.len() != 1 {
                return Err(SemaError::arity("macroexpand", "1", args.len()));
            }
            if let Some(items) = args[0].as_list() {
                if !items.is_empty() {
                    if let Some(spur) = items[0].as_symbol_spur() {
                        // Upgrade lazily: the non-macro passthrough below never
                        // touches the env.
                        let me_env = upgrade_delegate_env(&me_env)?;
                        if let Some(mac_val) = me_env.get(spur) {
                            if let Some(mac) = mac_val.as_macro_rc() {
                                if mac.syntax_rules.is_some() {
                                    return crate::syntax_rules::expand(&mac, &items[1..], &me_env);
                                }
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

    // __vm-deftool: the VM has already evaluated description/parameters/handler
    // and passes them as values, so build the tool directly.
    let tool_env = Rc::downgrade(env);
    env.set(
        intern("__vm-deftool"),
        Value::native_fn(NativeFn::simple("__vm-deftool", move |args| {
            if args.len() != 4 {
                return Err(SemaError::arity("deftool", "4", args.len()));
            }
            let name = args[0]
                .as_symbol()
                .ok_or_else(|| SemaError::eval("deftool: name must be a symbol"))?;
            let tool_env = upgrade_delegate_env(&tool_env)?;
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
    // agent directly.
    let agent_env = Rc::downgrade(env);
    env.set(
        intern("__vm-defagent"),
        Value::native_fn(NativeFn::simple("__vm-defagent", move |args| {
            if args.len() != 2 {
                return Err(SemaError::arity("defagent", "2", args.len()));
            }
            let name = args[0]
                .as_symbol()
                .ok_or_else(|| SemaError::eval("defagent: name must be a symbol"))?;
            let agent_env = upgrade_delegate_env(&agent_env)?;
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

    // __vm-match-failed: the strict `(match ...)` no-clause-matched path. Always
    // raises an :eval error carrying the unmatched value. `match*` never calls
    // this (it returns nil instead).
    env.set(
        intern("__vm-match-failed"),
        Value::native_fn(NativeFn::simple("__vm-match-failed", |args| {
            let val = args.first().cloned().unwrap_or_else(Value::nil);
            Err(
                SemaError::eval(format!("match: no clause matched value: {val}")).with_hint(
                    "add a catch-all `(_ ...)` clause, or use `match*` to return nil on no match",
                ),
            )
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

    // gc/collect: run a full cycle collection now (CORE-2). User-facing —
    // registered here (not sema-stdlib) because pin computation needs
    // sema-vm's current-VM introspection. Pins skip descent into the live
    // global namespace of the executing VM (or of this interpreter when
    // called outside one); correctness never depends on pins — live objects
    // are protected by their external strong counts.
    let gc_env = Rc::downgrade(env);
    env.set(
        intern("gc/collect"),
        Value::native_fn(NativeFn::simple("gc/collect", move |args| {
            if !args.is_empty() {
                return Err(SemaError::arity("gc/collect", "0", args.len()));
            }
            let pins = match sema_vm::current_vm_globals() {
                Some(globals) => sema_core::gc_env_chain_pins(&globals),
                None => match gc_env.upgrade() {
                    Some(env) => sema_core::gc_env_chain_pins(&env),
                    None => Vec::new(),
                },
            };
            Ok(gc_stats_map(&sema_core::gc_collect(
                &pins,
                sema_core::GcTrigger::Explicit,
            )))
        })),
    );

    // gc/stats: report the last completed collection's stats plus the current
    // candidate-registry size, without collecting.
    env.set(
        intern("gc/stats"),
        Value::native_fn(NativeFn::simple("gc/stats", |args| {
            if !args.is_empty() {
                return Err(SemaError::arity("gc/stats", "0", args.len()));
            }
            let mut map = gc_stats_btree(&sema_core::gc_last_stats());
            map.insert(
                Value::keyword("registry-size"),
                Value::int(sema_core::gc_registry_len() as i64),
            );
            Ok(Value::map(map))
        })),
    );
}

/// `{:candidates N :traced N :collected N :pruned N}` for the gc builtins.
fn gc_stats_btree(stats: &sema_core::GcStats) -> BTreeMap<Value, Value> {
    let mut map = BTreeMap::new();
    map.insert(
        Value::keyword("candidates"),
        Value::int(stats.candidates as i64),
    );
    map.insert(Value::keyword("traced"), Value::int(stats.traced as i64));
    map.insert(
        Value::keyword("collected"),
        Value::int(stats.collected as i64),
    );
    map.insert(Value::keyword("pruned"), Value::int(stats.pruned as i64));
    map
}

fn gc_stats_map(stats: &sema_core::GcStats) -> Value {
    Value::map(gc_stats_btree(stats))
}
