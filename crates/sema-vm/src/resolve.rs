use sema_core::{SemaError, Spur};

use crate::chunk::UpvalueDesc;
use crate::core_expr::{
    CoreExpr, DoLoop, DoVar, LambdaDef, PromptEntry, ResolvedExpr, VarRef, VarResolution,
};

/// Resolve all variable references in a CoreExpr tree.
/// Top-level expressions have no enclosing function scope, so all
/// variables that aren't in explicit let/define bindings become globals.
/// Returns the resolved expression and the number of top-level local slots needed.
pub fn resolve_with_locals(expr: &CoreExpr) -> Result<(ResolvedExpr, u16), SemaError> {
    let mut resolver = Resolver::new();
    let resolved = resolve_expr(expr, &mut resolver)?;
    let n_locals = resolver.scopes.last().unwrap().next_slot;
    Ok((resolved, n_locals))
}

/// A local variable in a scope.
#[derive(Debug, Clone)]
struct LocalVar {
    name: Spur,
    slot: u16,
    is_captured: bool,
}

/// A block scope within a function (e.g., let bindings).
#[derive(Debug)]
struct BlockScope {
    locals: Vec<LocalVar>,
}

/// A function scope (lambda or top-level).
#[derive(Debug)]
struct FunctionScope {
    blocks: Vec<BlockScope>,
    upvalues: Vec<UpvalueDesc>,
    upvalue_names: Vec<Spur>,
    next_slot: u16,
    is_top_level: bool,
}

impl FunctionScope {
    fn new(is_top_level: bool) -> Self {
        FunctionScope {
            blocks: vec![BlockScope { locals: Vec::new() }],
            upvalues: Vec::new(),
            upvalue_names: Vec::new(),
            next_slot: 0,
            is_top_level,
        }
    }

    fn define_local(&mut self, name: Spur) -> u16 {
        let slot = self.next_slot;
        self.next_slot += 1;
        self.blocks.last_mut().unwrap().locals.push(LocalVar {
            name,
            slot,
            is_captured: false,
        });
        slot
    }

    fn push_block(&mut self) {
        self.blocks.push(BlockScope { locals: Vec::new() });
    }

    fn pop_block(&mut self) {
        self.blocks.pop();
    }

    /// Find a local in this function's scope chain (innermost first).
    fn find_local(&self, name: Spur) -> Option<u16> {
        for block in self.blocks.iter().rev() {
            for local in block.locals.iter().rev() {
                if local.name == name {
                    return Some(local.slot);
                }
            }
        }
        None
    }

    /// Mark a local as captured (needed for upvalue boxing).
    fn mark_captured(&mut self, slot: u16) {
        for block in self.blocks.iter_mut() {
            for local in block.locals.iter_mut() {
                if local.slot == slot {
                    local.is_captured = true;
                    return;
                }
            }
        }
    }

    /// Add an upvalue, returning its index. Deduplicates.
    fn add_upvalue(&mut self, info: UpvalueDesc, name: Spur) -> u16 {
        // Check if we already capture this exact source
        for (i, existing) in self.upvalues.iter().enumerate() {
            match (existing, &info) {
                (UpvalueDesc::ParentLocal(a), UpvalueDesc::ParentLocal(b)) if a == b => {
                    return i as u16;
                }
                (UpvalueDesc::ParentUpvalue(a), UpvalueDesc::ParentUpvalue(b)) if a == b => {
                    return i as u16;
                }
                _ => {}
            }
        }
        let idx = self.upvalues.len() as u16;
        self.upvalues.push(info);
        self.upvalue_names.push(name);
        idx
    }
}

/// Maximum recursion depth for the resolver.
/// This prevents native stack overflow from deeply nested expressions.
const MAX_RESOLVE_DEPTH: usize = 256;

/// The resolver maintains a stack of function scopes.
struct Resolver {
    scopes: Vec<FunctionScope>,
    depth: usize,
}

impl Resolver {
    fn new() -> Self {
        Resolver {
            scopes: vec![FunctionScope::new(true)],
            depth: 0,
        }
    }

    fn current(&mut self) -> &mut FunctionScope {
        self.scopes.last_mut().unwrap()
    }

    fn define_local(&mut self, name: Spur) -> u16 {
        self.current().define_local(name)
    }

    fn push_block(&mut self) {
        self.current().push_block();
    }

    fn pop_block(&mut self) {
        self.current().pop_block();
    }

    fn push_function(&mut self) {
        self.scopes.push(FunctionScope::new(false));
    }

    fn pop_function(&mut self) -> FunctionScope {
        self.scopes.pop().unwrap()
    }

    /// Resolve a variable name. Search order:
    /// 1. Current function's locals (innermost block first)
    /// 2. Enclosing functions' locals (creating upvalue chain)
    /// 3. Global
    fn resolve_var(&mut self, name: Spur) -> VarResolution {
        let n = self.scopes.len();

        // 1. Check current function's locals
        if let Some(slot) = self.scopes[n - 1].find_local(name) {
            return VarResolution::Local { slot };
        }

        // 2. Walk enclosing scopes to find the variable as an upvalue
        if n > 1 {
            if let Some(uv_idx) = self.resolve_upvalue(n - 1, name) {
                return VarResolution::Upvalue { index: uv_idx };
            }
        }

        // 3. Global
        VarResolution::Global { spur: name }
    }

    /// Try to resolve `name` as an upvalue for the function at `scope_idx`.
    /// Standard Lua/Crafting Interpreters algorithm:
    /// - Check parent's locals → capture as ParentLocal
    /// - Else recurse into parent's upvalues → capture as ParentUpvalue
    /// - Top-level (scope 0) cannot be captured from → return None
    fn resolve_upvalue(&mut self, scope_idx: usize, name: Spur) -> Option<u16> {
        if scope_idx == 0 {
            return None; // top-level has no enclosing function to capture from
        }
        let parent_idx = scope_idx - 1;

        // Check parent function's locals
        if let Some(slot) = self.scopes[parent_idx].find_local(name) {
            self.scopes[parent_idx].mark_captured(slot);
            return Some(self.scopes[scope_idx].add_upvalue(UpvalueDesc::ParentLocal(slot), name));
        }

        // Recurse: check if parent can resolve it as an upvalue
        if let Some(parent_uv) = self.resolve_upvalue(parent_idx, name) {
            return Some(
                self.scopes[scope_idx].add_upvalue(UpvalueDesc::ParentUpvalue(parent_uv), name),
            );
        }

        None
    }
}

fn resolve_expr(expr: &CoreExpr, r: &mut Resolver) -> Result<ResolvedExpr, SemaError> {
    r.depth += 1;
    if r.depth > MAX_RESOLVE_DEPTH {
        r.depth -= 1;
        return Err(SemaError::eval("maximum resolution depth exceeded"));
    }
    let result = resolve_expr_inner(expr, r);
    r.depth -= 1;
    result
}

fn resolve_expr_inner(expr: &CoreExpr, r: &mut Resolver) -> Result<ResolvedExpr, SemaError> {
    match expr {
        CoreExpr::Const(v) => Ok(ResolvedExpr::Const(v.clone())),

        CoreExpr::Var(spur) => {
            let resolution = r.resolve_var(*spur);
            Ok(ResolvedExpr::Var(VarRef {
                name: *spur,
                resolution,
            }))
        }

        CoreExpr::If { test, then, else_ } => Ok(ResolvedExpr::If {
            test: Box::new(resolve_expr(test, r)?),
            then: Box::new(resolve_expr(then, r)?),
            else_: Box::new(resolve_expr(else_, r)?),
        }),

        CoreExpr::Begin(exprs) => {
            // Inside a function, pre-register all inner define names so they can
            // reference each other (letrec* semantics / R5RS internal defines).
            if !(r.current().is_top_level && r.current().blocks.len() == 1) {
                for expr in exprs {
                    let inner = match expr {
                        CoreExpr::Spanned(_, inner) => inner.as_ref(),
                        other => other,
                    };
                    if let CoreExpr::Define(spur, _) = inner {
                        if r.current().find_local(*spur).is_none() {
                            r.define_local(*spur);
                        }
                    }
                }
            }
            Ok(ResolvedExpr::Begin(resolve_exprs(exprs, r)?))
        }

        CoreExpr::Set(spur, expr) => {
            let val = resolve_expr(expr, r)?;
            let resolution = r.resolve_var(*spur);
            Ok(ResolvedExpr::Set(
                VarRef {
                    name: *spur,
                    resolution,
                },
                Box::new(val),
            ))
        }

        CoreExpr::Lambda(def) => resolve_lambda(def, r),

        CoreExpr::Call { func, args, tail } => Ok(ResolvedExpr::Call {
            func: Box::new(resolve_expr(func, r)?),
            args: resolve_exprs(args, r)?,
            tail: *tail,
        }),

        CoreExpr::Define(spur, expr) => {
            // At top level, define is global. Inside a function, define creates a local.
            if r.current().is_top_level && r.current().blocks.len() == 1 {
                let val = resolve_expr(expr, r)?;
                Ok(ResolvedExpr::Define(*spur, Box::new(val)))
            } else {
                // Inside a function or block: create a local binding BEFORE resolving RHS.
                // This allows recursive internal defines (the lambda body can reference
                // its own name via upvalue capture).
                // If already pre-registered by Begin's forward-scan, reuse that slot.
                let slot = r
                    .current()
                    .find_local(*spur)
                    .unwrap_or_else(|| r.define_local(*spur));
                let val = resolve_expr(expr, r)?;
                Ok(ResolvedExpr::Set(
                    VarRef {
                        name: *spur,
                        resolution: VarResolution::Local { slot },
                    },
                    Box::new(val),
                ))
            }
        }

        CoreExpr::Let { bindings, body } => resolve_let(bindings, body, r),
        CoreExpr::LetStar { bindings, body } => resolve_let_star(bindings, body, r),
        CoreExpr::Letrec { bindings, body } => resolve_letrec(bindings, body, r),
        // CoreExpr::NamedLet removed — desugared to Letrec+Lambda in lowering
        CoreExpr::Do(do_loop) => resolve_do(do_loop, r),

        CoreExpr::Try {
            body,
            catch_var,
            handler,
        } => resolve_try(body, *catch_var, handler, r),

        CoreExpr::Throw(expr) => Ok(ResolvedExpr::Throw(Box::new(resolve_expr(expr, r)?))),

        CoreExpr::And(exprs) => Ok(ResolvedExpr::And(resolve_exprs(exprs, r)?)),
        CoreExpr::Or(exprs) => Ok(ResolvedExpr::Or(resolve_exprs(exprs, r)?)),
        CoreExpr::Quote(v) => Ok(ResolvedExpr::Quote(v.clone())),

        CoreExpr::MakeList(exprs) => Ok(ResolvedExpr::MakeList(resolve_exprs(exprs, r)?)),
        CoreExpr::MakeVector(exprs) => Ok(ResolvedExpr::MakeVector(resolve_exprs(exprs, r)?)),
        CoreExpr::MakeMap(pairs) => {
            let resolved = pairs
                .iter()
                .map(|(k, v)| Ok((resolve_expr(k, r)?, resolve_expr(v, r)?)))
                .collect::<Result<_, SemaError>>()?;
            Ok(ResolvedExpr::MakeMap(resolved))
        }

        CoreExpr::Defmacro {
            name,
            params,
            rest,
            body,
        } => {
            // Defmacro body is resolved in a new function scope (the macro transformer)
            r.push_function();
            for p in params {
                r.define_local(*p);
            }
            if let Some(rest_param) = rest {
                r.define_local(*rest_param);
            }
            let resolved_body = resolve_body(body, r)?;
            let _fn_scope = r.pop_function();
            Ok(ResolvedExpr::Defmacro {
                name: *name,
                params: params.clone(),
                rest: *rest,
                body: resolved_body,
            })
        }

        CoreExpr::DefineRecordType {
            type_name,
            ctor_name,
            pred_name,
            field_names,
            field_specs,
        } => Ok(ResolvedExpr::DefineRecordType {
            type_name: *type_name,
            ctor_name: *ctor_name,
            pred_name: *pred_name,
            field_names: field_names.clone(),
            field_specs: field_specs.clone(),
        }),

        CoreExpr::Module {
            name,
            exports,
            body,
        } => Ok(ResolvedExpr::Module {
            name: *name,
            exports: exports.clone(),
            body: resolve_body(body, r)?,
        }),

        CoreExpr::Import { path, selective } => Ok(ResolvedExpr::Import {
            path: Box::new(resolve_expr(path, r)?),
            selective: selective.clone(),
        }),

        CoreExpr::Load(expr) => Ok(ResolvedExpr::Load(Box::new(resolve_expr(expr, r)?))),
        CoreExpr::Eval(expr) => Ok(ResolvedExpr::Eval(Box::new(resolve_expr(expr, r)?))),

        CoreExpr::Prompt(entries) => {
            let resolved = entries
                .iter()
                .map(|e| resolve_prompt_entry(e, r))
                .collect::<Result<_, _>>()?;
            Ok(ResolvedExpr::Prompt(resolved))
        }

        CoreExpr::Message { role, parts } => Ok(ResolvedExpr::Message {
            role: Box::new(resolve_expr(role, r)?),
            parts: resolve_exprs(parts, r)?,
        }),

        CoreExpr::Deftool {
            name,
            description,
            parameters,
            handler,
        } => Ok(ResolvedExpr::Deftool {
            name: *name,
            description: Box::new(resolve_expr(description, r)?),
            parameters: Box::new(resolve_expr(parameters, r)?),
            handler: Box::new(resolve_expr(handler, r)?),
        }),

        CoreExpr::Defagent { name, options } => Ok(ResolvedExpr::Defagent {
            name: *name,
            options: Box::new(resolve_expr(options, r)?),
        }),

        CoreExpr::Delay(expr) => Ok(ResolvedExpr::Delay(Box::new(resolve_expr(expr, r)?))),
        CoreExpr::Force(expr) => Ok(ResolvedExpr::Force(Box::new(resolve_expr(expr, r)?))),

        CoreExpr::Macroexpand(expr) => {
            Ok(ResolvedExpr::Macroexpand(Box::new(resolve_expr(expr, r)?)))
        }

        CoreExpr::Spanned(span, inner) => {
            let resolved = resolve_expr(inner, r)?;
            Ok(ResolvedExpr::Spanned(*span, Box::new(resolved)))
        }
    }
}

fn resolve_exprs(exprs: &[CoreExpr], r: &mut Resolver) -> Result<Vec<ResolvedExpr>, SemaError> {
    exprs.iter().map(|e| resolve_expr(e, r)).collect()
}

/// Resolve a body (lambda, let, letrec, etc.) with R5RS internal define semantics:
/// pre-register all inner define names so they can forward-reference each other.
fn resolve_body(exprs: &[CoreExpr], r: &mut Resolver) -> Result<Vec<ResolvedExpr>, SemaError> {
    if !(r.current().is_top_level && r.current().blocks.len() == 1) {
        for expr in exprs {
            let inner = match expr {
                CoreExpr::Spanned(_, inner) => inner.as_ref(),
                other => other,
            };
            if let CoreExpr::Define(spur, _) = inner {
                if r.current().find_local(*spur).is_none() {
                    r.define_local(*spur);
                }
            }
        }
    }
    resolve_exprs(exprs, r)
}

fn resolve_prompt_entry(
    entry: &PromptEntry<Spur>,
    r: &mut Resolver,
) -> Result<PromptEntry<VarRef>, SemaError> {
    match entry {
        PromptEntry::RoleContent { role, parts } => Ok(PromptEntry::RoleContent {
            role: role.clone(),
            parts: resolve_exprs(parts, r)?,
        }),
        PromptEntry::Expr(expr) => Ok(PromptEntry::Expr(resolve_expr(expr, r)?)),
    }
}

fn resolve_lambda(def: &LambdaDef<Spur>, r: &mut Resolver) -> Result<ResolvedExpr, SemaError> {
    r.push_function();

    // Define params as locals
    for param in &def.params {
        r.define_local(*param);
    }
    if let Some(rest) = def.rest {
        r.define_local(rest);
    }

    let body = resolve_body(&def.body, r)?;
    let fn_scope = r.pop_function();

    Ok(ResolvedExpr::Lambda(LambdaDef {
        name: def.name,
        params: def.params.clone(),
        rest: def.rest,
        body,
        upvalues: fn_scope.upvalues,
        upvalue_names: fn_scope.upvalue_names,
        n_locals: fn_scope.next_slot,
    }))
}

fn resolve_let(
    bindings: &[(Spur, CoreExpr)],
    body: &[CoreExpr],
    r: &mut Resolver,
) -> Result<ResolvedExpr, SemaError> {
    // Evaluate all inits in the outer scope first
    let mut inits = Vec::with_capacity(bindings.len());
    for (_, init_expr) in bindings {
        inits.push(resolve_expr(init_expr, r)?);
    }

    // Then define locals in a new block
    r.push_block();
    let mut resolved_bindings = Vec::with_capacity(bindings.len());
    for (name, _) in bindings {
        let slot = r.define_local(*name);
        resolved_bindings.push((
            VarRef {
                name: *name,
                resolution: VarResolution::Local { slot },
            },
            inits.remove(0),
        ));
    }

    let resolved_body = resolve_body(body, r)?;
    r.pop_block();

    Ok(ResolvedExpr::Let {
        bindings: resolved_bindings,
        body: resolved_body,
    })
}

fn resolve_let_star(
    bindings: &[(Spur, CoreExpr)],
    body: &[CoreExpr],
    r: &mut Resolver,
) -> Result<ResolvedExpr, SemaError> {
    r.push_block();
    let mut resolved_bindings = Vec::with_capacity(bindings.len());
    for (name, init_expr) in bindings {
        // Each init can see previous bindings
        let init = resolve_expr(init_expr, r)?;
        let slot = r.define_local(*name);
        resolved_bindings.push((
            VarRef {
                name: *name,
                resolution: VarResolution::Local { slot },
            },
            init,
        ));
    }
    let resolved_body = resolve_body(body, r)?;
    r.pop_block();

    Ok(ResolvedExpr::LetStar {
        bindings: resolved_bindings,
        body: resolved_body,
    })
}

fn resolve_letrec(
    bindings: &[(Spur, CoreExpr)],
    body: &[CoreExpr],
    r: &mut Resolver,
) -> Result<ResolvedExpr, SemaError> {
    r.push_block();
    // Define all binding names first (they can reference each other)
    let mut var_refs = Vec::with_capacity(bindings.len());
    for (name, _) in bindings {
        let slot = r.define_local(*name);
        var_refs.push(VarRef {
            name: *name,
            resolution: VarResolution::Local { slot },
        });
    }
    // Also pre-register body defines so letrec inits can reference them
    // (R5RS: internal defines in letrec body are visible to init expressions)
    for expr in body {
        let inner = match expr {
            CoreExpr::Spanned(_, inner) => inner.as_ref(),
            other => other,
        };
        if let CoreExpr::Define(spur, _) = inner {
            if r.current().find_local(*spur).is_none() {
                r.define_local(*spur);
            }
        }
    }
    // Then resolve inits (all names are in scope)
    let mut resolved_bindings = Vec::with_capacity(bindings.len());
    for (i, (_, init_expr)) in bindings.iter().enumerate() {
        let init = resolve_expr(init_expr, r)?;
        resolved_bindings.push((var_refs[i], init));
    }

    // Self-tail-call optimization (issue #62): a binding bound to a lambda that
    // references its own name only as a tail-call operator does not need to
    // capture itself — the running frame already holds its own closure. Eliding
    // the self upvalue removes the CORE-2 self-reference cycle (ADR #66).
    for (vr, value) in resolved_bindings.iter_mut() {
        if let VarResolution::Local { slot } = vr.resolution {
            optimize_self_tail(value, slot);
        }
    }

    let resolved_body = resolve_body(body, r)?;
    r.pop_block();

    Ok(ResolvedExpr::Letrec {
        bindings: resolved_bindings,
        body: resolved_body,
    })
}

/// If a resolved letrec binding value is a lambda whose only reference to its
/// own name (captured from enclosing local `slot`) is as the operator of a tail
/// call, elide that self upvalue and rewrite the self-calls to
/// `VarResolution::SelfFn`. No-op otherwise. See issue #62.
fn optimize_self_tail(value: &mut ResolvedExpr, slot: u16) {
    // Peek through a span wrapper to reach the lambda (user `letrec` inits are
    // spanned list forms; named-let inits are bare lambdas built in lowering).
    let lambda = match value {
        ResolvedExpr::Lambda(def) => def,
        ResolvedExpr::Spanned(_, inner) => match inner.as_mut() {
            ResolvedExpr::Lambda(def) => def,
            _ => return,
        },
        _ => return,
    };

    // The upvalue (if any) that captures the loop's own enclosing local slot.
    let Some(self_uv) = lambda
        .upvalues
        .iter()
        .position(|d| matches!(d, UpvalueDesc::ParentLocal(s) if *s == slot))
    else {
        return; // loop name never referenced from its own body
    };
    let self_uv = self_uv as u16;

    if !self_tail_only(&lambda.body, self_uv) {
        return; // the name escapes — keep the real self-capture
    }

    // Qualified: rewrite references, then drop the now-unused self upvalue.
    for e in &mut lambda.body {
        rewrite_self_refs(e, self_uv);
    }
    lambda.upvalues.remove(self_uv as usize);
    lambda.upvalue_names.remove(self_uv as usize);
}

/// True iff `vr` is a reference to the loop lambda's self upvalue.
fn is_self_upvalue(vr: &VarRef, self_uv: u16) -> bool {
    matches!(vr.resolution, VarResolution::Upvalue { index } if index == self_uv)
}

/// True iff a nested lambda captures the loop's self upvalue. The resolver
/// chains captures through every intermediate frame, so a *direct* child lambda
/// lists `ParentUpvalue(self_uv)` whenever any lambda at any depth references
/// the loop name — one check suffices.
fn lambda_captures_self(inner: &LambdaDef<VarRef>, self_uv: u16) -> bool {
    inner
        .upvalues
        .iter()
        .any(|d| matches!(d, UpvalueDesc::ParentUpvalue(i) if *i == self_uv))
}

/// True iff every reference to `self_uv` in `body` is the operator of a tail
/// call — the precondition for eliding the self upvalue.
fn self_tail_only(body: &[ResolvedExpr], self_uv: u16) -> bool {
    body.iter().all(|e| scan_self_tail(e, self_uv))
}

/// Returns false as soon as a disqualifying use of `self_uv` is found: used as a
/// value, `set!` target, non-tail-call operator, or captured by a nested lambda.
fn scan_self_tail(e: &ResolvedExpr, self_uv: u16) -> bool {
    use ResolvedExpr as E;
    match e {
        E::Var(vr) => !is_self_upvalue(vr, self_uv),
        E::Set(vr, val) => !is_self_upvalue(vr, self_uv) && scan_self_tail(val, self_uv),
        E::Call { func, args, tail } => {
            // The only approved use is the operator of a tail call: skip the
            // operator and check the arguments.
            let func_ok =
                if *tail && matches!(func.as_ref(), E::Var(vr) if is_self_upvalue(vr, self_uv)) {
                    true
                } else {
                    scan_self_tail(func, self_uv)
                };
            func_ok && args.iter().all(|a| scan_self_tail(a, self_uv))
        }
        E::Lambda(inner) => !lambda_captures_self(inner, self_uv),
        E::Const(_) | E::Quote(_) | E::DefineRecordType { .. } => true,
        E::If { test, then, else_ } => {
            scan_self_tail(test, self_uv)
                && scan_self_tail(then, self_uv)
                && scan_self_tail(else_, self_uv)
        }
        E::Begin(v) | E::And(v) | E::Or(v) | E::MakeList(v) | E::MakeVector(v) => {
            v.iter().all(|x| scan_self_tail(x, self_uv))
        }
        E::Define(_, val)
        | E::Throw(val)
        | E::Load(val)
        | E::Eval(val)
        | E::Delay(val)
        | E::Force(val)
        | E::Macroexpand(val)
        | E::Spanned(_, val) => scan_self_tail(val, self_uv),
        E::Let { bindings, body }
        | E::LetStar { bindings, body }
        | E::Letrec { bindings, body } => {
            bindings
                .iter()
                .all(|(_, init)| scan_self_tail(init, self_uv))
                && body.iter().all(|x| scan_self_tail(x, self_uv))
        }
        E::Do(do_loop) => {
            do_loop.vars.iter().all(|v| {
                scan_self_tail(&v.init, self_uv)
                    && v.step.as_ref().is_none_or(|s| scan_self_tail(s, self_uv))
            }) && scan_self_tail(&do_loop.test, self_uv)
                && do_loop.result.iter().all(|x| scan_self_tail(x, self_uv))
                && do_loop.body.iter().all(|x| scan_self_tail(x, self_uv))
        }
        E::Try { body, handler, .. } => {
            body.iter().all(|x| scan_self_tail(x, self_uv))
                && handler.iter().all(|x| scan_self_tail(x, self_uv))
        }
        E::MakeMap(pairs) => pairs
            .iter()
            .all(|(k, v)| scan_self_tail(k, self_uv) && scan_self_tail(v, self_uv)),
        E::Defmacro { body, .. } | E::Module { body, .. } => {
            body.iter().all(|x| scan_self_tail(x, self_uv))
        }
        E::Import { path, .. } => scan_self_tail(path, self_uv),
        E::Prompt(entries) => entries.iter().all(|entry| match entry {
            PromptEntry::RoleContent { parts, .. } => {
                parts.iter().all(|x| scan_self_tail(x, self_uv))
            }
            PromptEntry::Expr(x) => scan_self_tail(x, self_uv),
        }),
        E::Message { role, parts } => {
            scan_self_tail(role, self_uv) && parts.iter().all(|x| scan_self_tail(x, self_uv))
        }
        E::Deftool {
            description,
            parameters,
            handler,
            ..
        } => {
            scan_self_tail(description, self_uv)
                && scan_self_tail(parameters, self_uv)
                && scan_self_tail(handler, self_uv)
        }
        E::Defagent { options, .. } => scan_self_tail(options, self_uv),
    }
}

/// Remap a single reference during the rewrite: the self upvalue becomes
/// `SelfFn`; upvalues above it shift down one (their slot vanished); everything
/// else is untouched. `scan_self_tail` has already proven `self_uv` occurs only
/// as tail-call operators, so every `Upvalue{self_uv}` reached here is one.
fn remap_self_ref(vr: &mut VarRef, self_uv: u16) {
    if let VarResolution::Upvalue { index } = vr.resolution {
        if index == self_uv {
            vr.resolution = VarResolution::SelfFn;
        } else if index > self_uv {
            vr.resolution = VarResolution::Upvalue { index: index - 1 };
        }
    }
}

/// Shift a direct nested lambda's parent-upvalue descriptors to match the loop
/// lambda's shrunken upvalue list. Its body indexes its *own* upvalues, so we do
/// not recurse into it.
fn remap_nested_lambda(inner: &mut LambdaDef<VarRef>, self_uv: u16) {
    for d in &mut inner.upvalues {
        if let UpvalueDesc::ParentUpvalue(i) = d {
            if *i > self_uv {
                *d = UpvalueDesc::ParentUpvalue(*i - 1);
            }
        }
    }
}

/// Rewrite every reference in a qualified loop body: self-calls to `SelfFn`,
/// higher upvalues shifted down, nested lambda descriptors adjusted.
fn rewrite_self_refs(e: &mut ResolvedExpr, self_uv: u16) {
    use ResolvedExpr as E;
    match e {
        E::Var(vr) => remap_self_ref(vr, self_uv),
        E::Set(vr, val) => {
            remap_self_ref(vr, self_uv);
            rewrite_self_refs(val, self_uv);
        }
        // Stop at nested lambdas: only their parent-upvalue descriptors index
        // this frame's upvalue list; their bodies index their own.
        E::Lambda(inner) => remap_nested_lambda(inner, self_uv),
        E::Const(_) | E::Quote(_) | E::DefineRecordType { .. } => {}
        E::If { test, then, else_ } => {
            rewrite_self_refs(test, self_uv);
            rewrite_self_refs(then, self_uv);
            rewrite_self_refs(else_, self_uv);
        }
        E::Begin(v) | E::And(v) | E::Or(v) | E::MakeList(v) | E::MakeVector(v) => {
            for x in v {
                rewrite_self_refs(x, self_uv);
            }
        }
        E::Call { func, args, .. } => {
            rewrite_self_refs(func, self_uv);
            for a in args {
                rewrite_self_refs(a, self_uv);
            }
        }
        E::Define(_, val)
        | E::Throw(val)
        | E::Load(val)
        | E::Eval(val)
        | E::Delay(val)
        | E::Force(val)
        | E::Macroexpand(val)
        | E::Spanned(_, val) => rewrite_self_refs(val, self_uv),
        E::Let { bindings, body }
        | E::LetStar { bindings, body }
        | E::Letrec { bindings, body } => {
            for (_, init) in bindings {
                rewrite_self_refs(init, self_uv);
            }
            for x in body {
                rewrite_self_refs(x, self_uv);
            }
        }
        E::Do(do_loop) => {
            for v in &mut do_loop.vars {
                rewrite_self_refs(&mut v.init, self_uv);
                if let Some(s) = &mut v.step {
                    rewrite_self_refs(s, self_uv);
                }
            }
            rewrite_self_refs(&mut do_loop.test, self_uv);
            for x in &mut do_loop.result {
                rewrite_self_refs(x, self_uv);
            }
            for x in &mut do_loop.body {
                rewrite_self_refs(x, self_uv);
            }
        }
        E::Try { body, handler, .. } => {
            for x in body {
                rewrite_self_refs(x, self_uv);
            }
            for x in handler {
                rewrite_self_refs(x, self_uv);
            }
        }
        E::MakeMap(pairs) => {
            for (k, v) in pairs {
                rewrite_self_refs(k, self_uv);
                rewrite_self_refs(v, self_uv);
            }
        }
        E::Defmacro { body, .. } | E::Module { body, .. } => {
            for x in body {
                rewrite_self_refs(x, self_uv);
            }
        }
        E::Import { path, .. } => rewrite_self_refs(path, self_uv),
        E::Prompt(entries) => {
            for entry in entries {
                match entry {
                    PromptEntry::RoleContent { parts, .. } => {
                        for x in parts {
                            rewrite_self_refs(x, self_uv);
                        }
                    }
                    PromptEntry::Expr(x) => rewrite_self_refs(x, self_uv),
                }
            }
        }
        E::Message { role, parts } => {
            rewrite_self_refs(role, self_uv);
            for x in parts {
                rewrite_self_refs(x, self_uv);
            }
        }
        E::Deftool {
            description,
            parameters,
            handler,
            ..
        } => {
            rewrite_self_refs(description, self_uv);
            rewrite_self_refs(parameters, self_uv);
            rewrite_self_refs(handler, self_uv);
        }
        E::Defagent { options, .. } => rewrite_self_refs(options, self_uv),
    }
}

fn resolve_do(do_loop: &DoLoop<Spur>, r: &mut Resolver) -> Result<ResolvedExpr, SemaError> {
    // Evaluate all inits in the outer scope
    let mut inits = Vec::with_capacity(do_loop.vars.len());
    for var in &do_loop.vars {
        inits.push(resolve_expr(&var.init, r)?);
    }

    r.push_block();
    // Define loop variables
    let mut resolved_vars = Vec::with_capacity(do_loop.vars.len());
    for var in &do_loop.vars {
        let slot = r.define_local(var.name);
        resolved_vars.push((
            VarRef {
                name: var.name,
                resolution: VarResolution::Local { slot },
            },
            inits.remove(0),
            var.step.as_ref(),
        ));
    }

    let test = resolve_expr(&do_loop.test, r)?;
    let result = resolve_exprs(&do_loop.result, r)?;
    let body = resolve_body(&do_loop.body, r)?;

    // Resolve step expressions (they can reference loop variables)
    let mut final_vars = Vec::with_capacity(resolved_vars.len());
    for (var_ref, init, step) in resolved_vars {
        let resolved_step = match step {
            Some(s) => Some(resolve_expr(s, r)?),
            None => None,
        };
        final_vars.push(DoVar {
            name: var_ref,
            init,
            step: resolved_step,
        });
    }

    r.pop_block();

    Ok(ResolvedExpr::Do(DoLoop {
        vars: final_vars,
        test: Box::new(test),
        result,
        body,
    }))
}

fn resolve_try(
    body: &[CoreExpr],
    catch_var: Spur,
    handler: &[CoreExpr],
    r: &mut Resolver,
) -> Result<ResolvedExpr, SemaError> {
    let resolved_body = resolve_body(body, r)?;

    r.push_block();
    let slot = r.define_local(catch_var);
    let catch_ref = VarRef {
        name: catch_var,
        resolution: VarResolution::Local { slot },
    };
    let resolved_handler = resolve_body(handler, r)?;
    r.pop_block();

    Ok(ResolvedExpr::Try {
        body: resolved_body,
        catch_var: catch_ref,
        handler: resolved_handler,
    })
}

#[cfg(test)]
mod tests {
    use sema_core::intern;

    use super::*;
    use crate::lower::lower;

    fn lower_str(input: &str) -> CoreExpr {
        let val = sema_reader::read(input).unwrap();
        lower(&val, None).unwrap()
    }

    fn resolve_str(input: &str) -> ResolvedExpr {
        let core = lower_str(input);
        let (resolved, _) = resolve_with_locals(&core).unwrap();
        resolved
    }

    #[test]
    fn test_resolve_literal() {
        match resolve_str("42") {
            ResolvedExpr::Const(_) => {}
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_global_var() {
        let expr = resolve_str("x");
        match expr {
            ResolvedExpr::Var(vr) => {
                assert_eq!(vr.resolution, VarResolution::Global { spur: intern("x") });
            }
            other => panic!("expected Var, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_lambda_param_is_local() {
        let expr = resolve_str("(lambda (x) x)");
        match expr {
            ResolvedExpr::Lambda(def) => {
                assert_eq!(def.n_locals, 1);
                assert!(def.upvalues.is_empty());
                match &def.body[0] {
                    ResolvedExpr::Var(vr) => {
                        assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                    }
                    other => panic!("expected Var, got {other:?}"),
                }
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_lambda_multiple_params() {
        let expr = resolve_str("(lambda (a b c) b)");
        match expr {
            ResolvedExpr::Lambda(def) => {
                assert_eq!(def.n_locals, 3);
                match &def.body[0] {
                    ResolvedExpr::Var(vr) => {
                        assert_eq!(vr.resolution, VarResolution::Local { slot: 1 });
                    }
                    other => panic!("expected Var, got {other:?}"),
                }
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_upvalue_simple() {
        // (lambda (x) (lambda () x)) — inner x is upvalue
        let expr = resolve_str("(lambda (x) (lambda () x))");
        match expr {
            ResolvedExpr::Lambda(outer) => {
                assert_eq!(outer.n_locals, 1);
                match &outer.body[0] {
                    ResolvedExpr::Lambda(inner) => {
                        assert_eq!(inner.upvalues.len(), 1);
                        assert!(matches!(inner.upvalues[0], UpvalueDesc::ParentLocal(0)));
                        match &inner.body[0] {
                            ResolvedExpr::Var(vr) => {
                                assert_eq!(vr.resolution, VarResolution::Upvalue { index: 0 });
                            }
                            other => panic!("expected Var, got {other:?}"),
                        }
                    }
                    other => panic!("expected inner Lambda, got {other:?}"),
                }
            }
            other => panic!("expected outer Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_upvalue_chain() {
        // (lambda (x) (lambda () (lambda () x)))
        // Innermost x captures through two levels
        let expr = resolve_str("(lambda (x) (lambda () (lambda () x)))");
        match expr {
            ResolvedExpr::Lambda(l1) => match &l1.body[0] {
                ResolvedExpr::Lambda(l2) => {
                    assert_eq!(l2.upvalues.len(), 1);
                    assert!(matches!(l2.upvalues[0], UpvalueDesc::ParentLocal(0)));
                    match &l2.body[0] {
                        ResolvedExpr::Lambda(l3) => {
                            assert_eq!(l3.upvalues.len(), 1);
                            assert!(matches!(l3.upvalues[0], UpvalueDesc::ParentUpvalue(0)));
                            match &l3.body[0] {
                                ResolvedExpr::Var(vr) => {
                                    assert_eq!(vr.resolution, VarResolution::Upvalue { index: 0 });
                                }
                                other => panic!("expected Var, got {other:?}"),
                            }
                        }
                        other => panic!("expected l3, got {other:?}"),
                    }
                }
                other => panic!("expected l2, got {other:?}"),
            },
            other => panic!("expected l1, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_global_in_call() {
        // (+ 1 2) → + is global
        let expr = resolve_str("(+ 1 2)");
        match expr {
            ResolvedExpr::Call { func, .. } => match *func {
                ResolvedExpr::Var(vr) => {
                    assert_eq!(vr.resolution, VarResolution::Global { spur: intern("+") });
                }
                other => panic!("expected Var for func, got {other:?}"),
            },
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_let_bindings() {
        let expr = resolve_str("(lambda () (let ((x 1) (y 2)) (+ x y)))");
        match expr {
            ResolvedExpr::Lambda(def) => {
                match &def.body[0] {
                    ResolvedExpr::Let { bindings, body } => {
                        assert_eq!(bindings.len(), 2);
                        assert_eq!(bindings[0].0.resolution, VarResolution::Local { slot: 0 });
                        assert_eq!(bindings[1].0.resolution, VarResolution::Local { slot: 1 });
                        // Body references should be locals
                        match &body[0] {
                            ResolvedExpr::Call { args, .. } => match &args[0] {
                                ResolvedExpr::Var(vr) => {
                                    assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                                }
                                other => panic!("expected Var, got {other:?}"),
                            },
                            other => panic!("expected Call, got {other:?}"),
                        }
                    }
                    other => panic!("expected Let, got {other:?}"),
                }
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_let_star_sequential() {
        // In let*, y can reference x
        let expr = resolve_str("(lambda () (let* ((x 1) (y x)) y))");
        match expr {
            ResolvedExpr::Lambda(def) => match &def.body[0] {
                ResolvedExpr::LetStar { bindings, .. } => {
                    // y's init is x which should resolve to local slot 0
                    match &bindings[1].1 {
                        ResolvedExpr::Var(vr) => {
                            assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                        }
                        other => panic!("expected Var, got {other:?}"),
                    }
                }
                other => panic!("expected LetStar, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_letrec() {
        // In letrec, bindings can reference each other
        let expr =
            resolve_str("(lambda () (letrec ((f (lambda () (g))) (g (lambda () (f)))) (f)))");
        match expr {
            ResolvedExpr::Lambda(def) => match &def.body[0] {
                ResolvedExpr::Letrec { bindings, .. } => {
                    assert_eq!(bindings.len(), 2);
                    // f and g are both locals
                    assert_eq!(bindings[0].0.resolution, VarResolution::Local { slot: 0 });
                    assert_eq!(bindings[1].0.resolution, VarResolution::Local { slot: 1 });
                }
                other => panic!("expected Letrec, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_set_local() {
        let expr = resolve_str("(lambda (x) (set! x 42))");
        match expr {
            ResolvedExpr::Lambda(def) => match &def.body[0] {
                ResolvedExpr::Set(vr, _) => {
                    assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                }
                other => panic!("expected Set, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_set_global() {
        let expr = resolve_str("(set! x 42)");
        match expr {
            ResolvedExpr::Set(vr, _) => {
                assert_eq!(vr.resolution, VarResolution::Global { spur: intern("x") });
            }
            other => panic!("expected Set, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_try_catch_var() {
        let expr = resolve_str("(lambda () (try (/ 1 0) (catch e (list e))))");
        match expr {
            ResolvedExpr::Lambda(def) => match &def.body[0] {
                ResolvedExpr::Try {
                    catch_var, handler, ..
                } => {
                    assert_eq!(catch_var.resolution, VarResolution::Local { slot: 0 });
                    // The handler body should reference e as the same slot
                    match &handler[0] {
                        ResolvedExpr::Call { args, .. } => match &args[0] {
                            ResolvedExpr::Var(vr) => {
                                assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                            }
                            other => panic!("expected Var, got {other:?}"),
                        },
                        other => panic!("expected Call, got {other:?}"),
                    }
                }
                other => panic!("expected Try, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_do_loop() {
        let expr = resolve_str("(lambda () (do ((i 0 (+ i 1))) ((= i 10) i) (display i)))");
        match expr {
            ResolvedExpr::Lambda(def) => match &def.body[0] {
                ResolvedExpr::Do(do_loop) => {
                    assert_eq!(do_loop.vars.len(), 1);
                    assert!(matches!(
                        do_loop.vars[0].name.resolution,
                        VarResolution::Local { slot: 0 }
                    ));
                    // Step expression must exist and reference i as local
                    match do_loop.vars[0].step.as_ref().expect("step must exist") {
                        ResolvedExpr::Call { args, .. } => match &args[0] {
                            ResolvedExpr::Var(vr) => {
                                assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                            }
                            other => panic!("expected Var in step, got {other:?}"),
                        },
                        other => panic!("expected Call in step, got {other:?}"),
                    }
                }
                other => panic!("expected Do, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_named_let() {
        // Named let desugars to letrec+lambda, so we get Letrec with a Lambda binding
        let expr = resolve_str("(lambda () (let loop ((n 10)) (if (= n 0) n (loop (- n 1)))))");
        match expr {
            ResolvedExpr::Lambda(def) => match &def.body[0] {
                ResolvedExpr::Letrec { bindings, body } => {
                    // loop is the letrec binding
                    assert_eq!(bindings.len(), 1);
                    // The binding value should be a Lambda
                    assert!(matches!(&bindings[0].1, ResolvedExpr::Lambda(_)));
                    // The letrec body should be a call to loop with initial values
                    assert_eq!(body.len(), 1);
                    assert!(matches!(&body[0], ResolvedExpr::Call { .. }));
                }
                other => panic!("expected Letrec, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_define_at_top_level() {
        // Top-level define stays as Define (global)
        let expr = resolve_str("(define x 42)");
        match expr {
            ResolvedExpr::Define(spur, _) => {
                assert_eq!(spur, intern("x"));
            }
            other => panic!("expected Define, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_define_in_function() {
        // Define inside a function becomes a local Set
        let expr = resolve_str("(lambda () (define x 42) x)");
        match expr {
            ResolvedExpr::Lambda(def) => {
                // First body expr should be Set to local
                match &def.body[0] {
                    ResolvedExpr::Set(vr, _) => {
                        assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                    }
                    other => panic!("expected Set for internal define, got {other:?}"),
                }
                // Second body expr references x as local
                match &def.body[1] {
                    ResolvedExpr::Var(vr) => {
                        assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                    }
                    other => panic!("expected Var, got {other:?}"),
                }
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_rest_param() {
        let expr = resolve_str("(lambda (x . rest) rest)");
        match expr {
            ResolvedExpr::Lambda(def) => {
                assert_eq!(def.n_locals, 2); // x=0, rest=1
                match &def.body[0] {
                    ResolvedExpr::Var(vr) => {
                        assert_eq!(vr.resolution, VarResolution::Local { slot: 1 });
                    }
                    other => panic!("expected Var, got {other:?}"),
                }
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_shadowing() {
        // Inner let shadows outer param
        let expr = resolve_str("(lambda (x) (let ((x 99)) x))");
        match expr {
            ResolvedExpr::Lambda(def) => match &def.body[0] {
                ResolvedExpr::Let { bindings, body } => {
                    // Shadowed x gets a new slot
                    assert_eq!(bindings[0].0.resolution, VarResolution::Local { slot: 1 });
                    // Body x refers to the shadowed slot
                    match &body[0] {
                        ResolvedExpr::Var(vr) => {
                            assert_eq!(vr.resolution, VarResolution::Local { slot: 1 });
                        }
                        other => panic!("expected Var, got {other:?}"),
                    }
                }
                other => panic!("expected Let, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_multiple_upvalues() {
        // (lambda (a b) (lambda () (+ a b)))
        let expr = resolve_str("(lambda (a b) (lambda () (+ a b)))");
        match expr {
            ResolvedExpr::Lambda(outer) => match &outer.body[0] {
                ResolvedExpr::Lambda(inner) => {
                    assert_eq!(inner.upvalues.len(), 2);
                    assert!(matches!(inner.upvalues[0], UpvalueDesc::ParentLocal(0)));
                    assert!(matches!(inner.upvalues[1], UpvalueDesc::ParentLocal(1)));
                }
                other => panic!("expected inner Lambda, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_upvalue_dedup() {
        // Same variable referenced twice in inner lambda → same upvalue index
        let expr = resolve_str("(lambda (x) (lambda () (+ x x)))");
        match expr {
            ResolvedExpr::Lambda(outer) => match &outer.body[0] {
                ResolvedExpr::Lambda(inner) => {
                    assert_eq!(inner.upvalues.len(), 1); // deduplicated
                    match &inner.body[0] {
                        ResolvedExpr::Call { args, .. } => match (&args[0], &args[1]) {
                            (ResolvedExpr::Var(a), ResolvedExpr::Var(b)) => {
                                assert_eq!(a.resolution, VarResolution::Upvalue { index: 0 });
                                assert_eq!(b.resolution, VarResolution::Upvalue { index: 0 });
                            }
                            other => panic!("expected Var args, got {other:?}"),
                        },
                        other => panic!("expected Call, got {other:?}"),
                    }
                }
                other => panic!("expected inner Lambda, got {other:?}"),
            },
            other => panic!("expected outer Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_quote_untouched() {
        let expr = resolve_str("'(x y z)");
        match expr {
            ResolvedExpr::Quote(_) => {}
            other => panic!("expected Quote, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_and_or() {
        let expr = resolve_str("(lambda (a b) (and a b))");
        match expr {
            ResolvedExpr::Lambda(def) => match &def.body[0] {
                ResolvedExpr::And(exprs) => {
                    assert_eq!(exprs.len(), 2);
                    match &exprs[0] {
                        ResolvedExpr::Var(vr) => {
                            assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                        }
                        other => panic!("expected Var, got {other:?}"),
                    }
                }
                other => panic!("expected And, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_set_through_upvalue() {
        // (lambda (x) (lambda () (set! x 1) x)) — set! targets upvalue
        let expr = resolve_str("(lambda (x) (lambda () (set! x 1) x))");
        match expr {
            ResolvedExpr::Lambda(outer) => match &outer.body[0] {
                ResolvedExpr::Lambda(inner) => {
                    assert_eq!(inner.upvalues.len(), 1);
                    assert!(matches!(inner.upvalues[0], UpvalueDesc::ParentLocal(0)));
                    // set! should target the upvalue
                    match &inner.body[0] {
                        ResolvedExpr::Set(vr, _) => {
                            assert_eq!(vr.resolution, VarResolution::Upvalue { index: 0 });
                        }
                        other => panic!("expected Set, got {other:?}"),
                    }
                    // var ref should also be upvalue
                    match &inner.body[1] {
                        ResolvedExpr::Var(vr) => {
                            assert_eq!(vr.resolution, VarResolution::Upvalue { index: 0 });
                        }
                        other => panic!("expected Var, got {other:?}"),
                    }
                }
                other => panic!("expected inner Lambda, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_deep_upvalue_chain_4_levels() {
        // 4 levels of nesting: x captured through 3 intermediate lambdas
        let expr = resolve_str("(lambda (x) (lambda () (lambda () (lambda () x))))");
        match expr {
            ResolvedExpr::Lambda(l1) => match &l1.body[0] {
                ResolvedExpr::Lambda(l2) => {
                    assert_eq!(l2.upvalues.len(), 1);
                    assert!(matches!(l2.upvalues[0], UpvalueDesc::ParentLocal(0)));
                    match &l2.body[0] {
                        ResolvedExpr::Lambda(l3) => {
                            assert_eq!(l3.upvalues.len(), 1);
                            assert!(matches!(l3.upvalues[0], UpvalueDesc::ParentUpvalue(0)));
                            match &l3.body[0] {
                                ResolvedExpr::Lambda(l4) => {
                                    assert_eq!(l4.upvalues.len(), 1);
                                    assert!(matches!(
                                        l4.upvalues[0],
                                        UpvalueDesc::ParentUpvalue(0)
                                    ));
                                    match &l4.body[0] {
                                        ResolvedExpr::Var(vr) => {
                                            assert_eq!(
                                                vr.resolution,
                                                VarResolution::Upvalue { index: 0 }
                                            );
                                        }
                                        other => panic!("expected Var, got {other:?}"),
                                    }
                                }
                                other => panic!("expected l4, got {other:?}"),
                            }
                        }
                        other => panic!("expected l3, got {other:?}"),
                    }
                }
                other => panic!("expected l2, got {other:?}"),
            },
            other => panic!("expected l1, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_shadowing_with_capture() {
        // Inner let shadows param, innermost lambda captures the let-bound x
        let expr = resolve_str("(lambda (x) (let ((x 2)) (lambda () x)))");
        match expr {
            ResolvedExpr::Lambda(outer) => match &outer.body[0] {
                ResolvedExpr::Let { bindings, body } => {
                    // let-bound x is slot 1 (param x is slot 0)
                    assert_eq!(bindings[0].0.resolution, VarResolution::Local { slot: 1 });
                    match &body[0] {
                        ResolvedExpr::Lambda(inner) => {
                            // Should capture the let-bound x (slot 1), not the param (slot 0)
                            assert_eq!(inner.upvalues.len(), 1);
                            assert!(matches!(inner.upvalues[0], UpvalueDesc::ParentLocal(1)));
                        }
                        other => panic!("expected Lambda, got {other:?}"),
                    }
                }
                other => panic!("expected Let, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_multiple_let_blocks_monotonic_slots() {
        // Two let blocks in same function — slots are monotonically allocated
        let expr = resolve_str("(lambda () (let ((x 1)) x) (let ((y 2)) y))");
        match expr {
            ResolvedExpr::Lambda(def) => {
                // x gets slot 0, y gets slot 1 (monotonic, no reuse)
                match &def.body[0] {
                    ResolvedExpr::Let { bindings, body } => {
                        assert_eq!(bindings[0].0.resolution, VarResolution::Local { slot: 0 });
                        match &body[0] {
                            ResolvedExpr::Var(vr) => {
                                assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                            }
                            other => panic!("expected Var, got {other:?}"),
                        }
                    }
                    other => panic!("expected Let, got {other:?}"),
                }
                match &def.body[1] {
                    ResolvedExpr::Let { bindings, body } => {
                        assert_eq!(bindings[0].0.resolution, VarResolution::Local { slot: 1 });
                        match &body[0] {
                            ResolvedExpr::Var(vr) => {
                                assert_eq!(vr.resolution, VarResolution::Local { slot: 1 });
                            }
                            other => panic!("expected Var, got {other:?}"),
                        }
                    }
                    other => panic!("expected Let, got {other:?}"),
                }
                assert_eq!(def.n_locals, 2);
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_nested_define_then_capture() {
        // (lambda () (define x 1) (lambda () x))
        // Internal define creates local, inner lambda captures it
        let expr = resolve_str("(lambda () (define x 1) (lambda () x))");
        match expr {
            ResolvedExpr::Lambda(outer) => {
                // define creates local slot 0
                match &outer.body[0] {
                    ResolvedExpr::Set(vr, _) => {
                        assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                    }
                    other => panic!("expected Set for define, got {other:?}"),
                }
                // Inner lambda captures x as upvalue
                match &outer.body[1] {
                    ResolvedExpr::Lambda(inner) => {
                        assert_eq!(inner.upvalues.len(), 1);
                        assert!(matches!(inner.upvalues[0], UpvalueDesc::ParentLocal(0)));
                        match &inner.body[0] {
                            ResolvedExpr::Var(vr) => {
                                assert_eq!(vr.resolution, VarResolution::Upvalue { index: 0 });
                            }
                            other => panic!("expected Var, got {other:?}"),
                        }
                    }
                    other => panic!("expected Lambda, got {other:?}"),
                }
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_top_level_var_is_global() {
        // Variables at top-level that aren't in bindings are global
        // even if defined with define at top level
        let expr = resolve_str("(lambda () x)");
        match expr {
            ResolvedExpr::Lambda(def) => match &def.body[0] {
                ResolvedExpr::Var(vr) => {
                    assert_eq!(vr.resolution, VarResolution::Global { spur: intern("x") });
                }
                other => panic!("expected Var, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_named_let_capture() {
        // Named let desugars to letrec+lambda; inner lambda captures n via upvalue chain
        let expr = resolve_str("(lambda () (let loop ((n 3)) (lambda () n)))");
        match expr {
            ResolvedExpr::Lambda(outer) => match &outer.body[0] {
                ResolvedExpr::Letrec { bindings, .. } => {
                    // The letrec binding should be a Lambda (the loop function)
                    match &bindings[0].1 {
                        ResolvedExpr::Lambda(loop_fn) => {
                            // The loop body contains (lambda () n) which captures n
                            match &loop_fn.body[0] {
                                ResolvedExpr::Lambda(inner) => {
                                    assert_eq!(inner.upvalues.len(), 1);
                                }
                                other => panic!("expected inner Lambda, got {other:?}"),
                            }
                        }
                        other => panic!("expected loop Lambda, got {other:?}"),
                    }
                }
                other => panic!("expected Letrec, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_module_body() {
        // Module body resolves variables — define is top-level global,
        // lambda param is local
        let expr = resolve_str("(module mymod (export f) (define f (lambda (x) x)))");
        match expr {
            ResolvedExpr::Module { name, body, .. } => {
                assert_eq!(name, intern("mymod"));
                // define f at module top level stays as Define (global)
                match &body[0] {
                    ResolvedExpr::Define(spur, val) => {
                        assert_eq!(*spur, intern("f"));
                        // The lambda's param x should be local slot 0
                        match val.as_ref() {
                            ResolvedExpr::Lambda(def) => {
                                assert_eq!(def.n_locals, 1);
                                match &def.body[0] {
                                    ResolvedExpr::Var(vr) => {
                                        assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                                    }
                                    other => panic!("expected Var, got {other:?}"),
                                }
                            }
                            other => panic!("expected Lambda, got {other:?}"),
                        }
                    }
                    other => panic!("expected Define, got {other:?}"),
                }
            }
            other => panic!("expected Module, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_call_args() {
        // (list x x) is a function call — args should resolve to locals
        let expr = resolve_str("(lambda (x) (list x x))");
        match expr {
            ResolvedExpr::Lambda(def) => match &def.body[0] {
                ResolvedExpr::Call { func, args, .. } => {
                    // list is a global
                    match func.as_ref() {
                        ResolvedExpr::Var(vr) => {
                            assert_eq!(
                                vr.resolution,
                                VarResolution::Global {
                                    spur: intern("list")
                                }
                            );
                        }
                        other => panic!("expected Var for func, got {other:?}"),
                    }
                    // both args are local slot 0
                    assert_eq!(args.len(), 2);
                    for arg in args {
                        match arg {
                            ResolvedExpr::Var(vr) => {
                                assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                            }
                            other => panic!("expected Var, got {other:?}"),
                        }
                    }
                }
                other => panic!("expected Call, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_make_vector_literal() {
        // [x y] in source is a vector literal — lowered to MakeVector
        let expr = resolve_str("(lambda (x y) [x y])");
        match expr {
            ResolvedExpr::Lambda(def) => match &def.body[0] {
                ResolvedExpr::MakeVector(elems) => {
                    assert_eq!(elems.len(), 2);
                    match &elems[0] {
                        ResolvedExpr::Var(vr) => {
                            assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                        }
                        other => panic!("expected Var, got {other:?}"),
                    }
                    match &elems[1] {
                        ResolvedExpr::Var(vr) => {
                            assert_eq!(vr.resolution, VarResolution::Local { slot: 1 });
                        }
                        other => panic!("expected Var, got {other:?}"),
                    }
                }
                other => panic!("expected MakeVector, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_depth_limit() {
        // Run on a thread with a larger stack to avoid native stack overflow
        // from deeply nested CoreExpr construction/drop.
        let result = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut expr = CoreExpr::Const(sema_core::Value::int(1));
                for _ in 0..300 {
                    expr = CoreExpr::Begin(vec![expr]);
                }
                let result = resolve_with_locals(&expr);
                assert!(result.is_err());
                let err = result.unwrap_err().to_string();
                assert!(
                    err.contains("resolution depth"),
                    "expected resolution depth error, got: {err}"
                );
            })
            .unwrap()
            .join();
        result.unwrap();
    }

    // ---- Tests verifying the returned local count from resolve_with_locals ----

    #[test]
    fn test_resolve_top_level_no_locals() {
        // A bare literal or global reference needs 0 top-level locals
        let core = lower_str("42");
        let (_, n_locals) = resolve_with_locals(&core).unwrap();
        assert_eq!(n_locals, 0, "bare literal should need 0 locals");
    }

    #[test]
    fn test_resolve_top_level_let_locals() {
        // (let ((x 1) (y 2)) (+ x y)) needs 2 top-level locals
        let core = lower_str("(let ((x 1) (y 2)) (+ x y))");
        let (_, n_locals) = resolve_with_locals(&core).unwrap();
        assert_eq!(n_locals, 2, "let with 2 bindings should need 2 locals");
    }

    #[test]
    fn test_resolve_top_level_nested_let_locals() {
        // Nested lets: outer has 1, inner adds 1. Slots can be reused
        // so it depends on the implementation — just verify it's >= 1
        let core = lower_str("(let ((x 1)) (let ((y 2)) (+ x y)))");
        let (_, n_locals) = resolve_with_locals(&core).unwrap();
        assert!(
            n_locals >= 2,
            "nested let should need at least 2 locals, got {n_locals}"
        );
    }

    #[test]
    fn test_resolve_top_level_define_local() {
        // Top-level define creates a local slot
        let core = lower_str("(define x 42)");
        let (_, n_locals) = resolve_with_locals(&core).unwrap();
        // define at top level is DefineGlobal which uses 0 locals
        // (the slot is in the global env, not in local slots)
        assert_eq!(
            n_locals, 0,
            "top-level define should use 0 local slots (it's global)"
        );
    }

    #[test]
    fn test_resolve_lambda_does_not_add_top_level_locals() {
        // A lambda definition doesn't allocate top-level locals
        // (it has its own scope)
        let core = lower_str("(fn (a b c) (+ a b c))");
        let (_, n_locals) = resolve_with_locals(&core).unwrap();
        assert_eq!(
            n_locals, 0,
            "lambda params should not count as top-level locals"
        );
    }

    #[test]
    fn test_resolve_inner_define_forward_reference() {
        // (lambda () (define (a) (b)) (define (b) 42) (a))
        // 'a' references 'b' which is defined after 'a' — forward reference.
        // Both should resolve as locals, not globals.
        let expr = resolve_str("(lambda () (define (a) (b)) (define (b) 42) (a))");
        match expr {
            ResolvedExpr::Lambda(def) => {
                // a=slot 0, b=slot 1, both pre-registered
                assert!(
                    def.n_locals >= 2,
                    "should have at least 2 locals for a and b"
                );
                // First body expr: set a = lambda that calls b
                match &def.body[0] {
                    ResolvedExpr::Set(vr, val) => {
                        assert_eq!(vr.resolution, VarResolution::Local { slot: 0 });
                        // The lambda body calls b — b should be an upvalue (captured from parent)
                        match val.as_ref() {
                            ResolvedExpr::Lambda(inner) => {
                                // b is captured as upvalue from parent scope
                                assert!(
                                    !inner.upvalues.is_empty(),
                                    "inner lambda should capture b"
                                );
                            }
                            other => panic!("expected Lambda for a's body, got {other:?}"),
                        }
                    }
                    other => panic!("expected Set for define a, got {other:?}"),
                }
                // Second body expr: set b = 42
                match &def.body[1] {
                    ResolvedExpr::Set(vr, _) => {
                        assert_eq!(vr.resolution, VarResolution::Local { slot: 1 });
                    }
                    other => panic!("expected Set for define b, got {other:?}"),
                }
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    // ---- Self-tail-call optimization (issue #62) ----

    /// Extract the loop lambda from a top-level named-let source.
    fn loop_lambda(src: &str) -> LambdaDef<VarRef> {
        match resolve_str(src) {
            ResolvedExpr::Letrec { bindings, .. } => match &bindings[0].1 {
                ResolvedExpr::Lambda(def) => def.clone(),
                other => panic!("expected Lambda binding, got {other:?}"),
            },
            other => panic!("expected Letrec, got {other:?}"),
        }
    }

    /// The func of the tail self-call in `(if (= n 0) <base> (loop <step>))`.
    fn self_call_resolution(loop_fn: &LambdaDef<VarRef>) -> VarResolution {
        match &loop_fn.body[0] {
            ResolvedExpr::If { else_, .. } => match else_.as_ref() {
                ResolvedExpr::Call { func, .. } => match func.as_ref() {
                    ResolvedExpr::Var(vr) => vr.resolution,
                    other => panic!("expected Var func, got {other:?}"),
                },
                other => panic!("expected Call in else branch, got {other:?}"),
            },
            other => panic!("expected If body, got {other:?}"),
        }
    }

    #[test]
    fn test_self_tail_call_named_let_elides_self_upvalue() {
        // Counter loop: `loop` is referenced only as a tail-call operator, so the
        // self upvalue is elided and the call resolves to SelfFn (no cycle).
        let loop_fn = loop_lambda("(let loop ((n 5)) (if (= n 0) n (loop (- n 1))))");
        assert!(
            loop_fn.upvalues.is_empty(),
            "self upvalue should be elided, got {:?}",
            loop_fn.upvalues
        );
        assert_eq!(self_call_resolution(&loop_fn), VarResolution::SelfFn);
    }

    #[test]
    fn test_self_tail_call_not_applied_when_loop_name_escapes() {
        // `loop` passed as a value (to `list`) must keep the real self upvalue —
        // the closure can be invoked from outside its own frame.
        let loop_fn = loop_lambda("(let loop ((n 5)) (if (= n 0) (list loop) (loop (- n 1))))");
        assert_eq!(
            loop_fn.upvalues.len(),
            1,
            "escaping loop name keeps its self upvalue"
        );
        assert!(matches!(
            self_call_resolution(&loop_fn),
            VarResolution::Upvalue { .. }
        ));
    }

    #[test]
    fn test_self_tail_call_not_applied_for_non_tail_self_call() {
        // Non-tail self-call `(+ 1 (loop ...))` disqualifies the optimization.
        let loop_fn = loop_lambda("(let loop ((n 5)) (if (= n 0) 0 (+ 1 (loop (- n 1)))))");
        assert_eq!(
            loop_fn.upvalues.len(),
            1,
            "non-tail self-call keeps the self upvalue"
        );
    }

    #[test]
    fn test_self_tail_call_not_applied_when_captured_by_inner_lambda() {
        // `loop` captured by a nested lambda needs a real upvalue (a different
        // frame runs the inner lambda), so the opt must not fire.
        let loop_fn =
            loop_lambda("(let loop ((n 5)) (if (= n 0) (lambda () (loop 0)) (loop (- n 1))))");
        assert_eq!(
            loop_fn.upvalues.len(),
            1,
            "inner-lambda capture keeps the self upvalue"
        );
    }

    #[test]
    fn test_resolve_inner_define_mutual_recursion() {
        // Mutually recursive inner defines — both reference each other
        let expr = resolve_str(
            "(lambda (n)
               (define (even? x) (if (= x 0) #t (odd? (- x 1))))
               (define (odd? x) (if (= x 0) #f (even? (- x 1))))
               (even? n))",
        );
        match expr {
            ResolvedExpr::Lambda(def) => {
                // n=0, even?=1, odd?=2
                assert!(def.n_locals >= 3);
                // even? and odd? should both be locals (not globals)
                match &def.body[0] {
                    ResolvedExpr::Set(vr, _) => {
                        assert!(matches!(vr.resolution, VarResolution::Local { .. }));
                    }
                    other => panic!("expected Set for even?, got {other:?}"),
                }
                match &def.body[1] {
                    ResolvedExpr::Set(vr, _) => {
                        assert!(matches!(vr.resolution, VarResolution::Local { .. }));
                    }
                    other => panic!("expected Set for odd?, got {other:?}"),
                }
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }
}
