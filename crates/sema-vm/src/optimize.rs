//! Constant folding and simplification pass on CoreExpr.
//!
//! Runs after lowering, before variable resolution. Folds:
//! - Arithmetic on constants: (+ 1 2) → 3
//! - Boolean simplification: (not #t) → #f
//! - If with constant test: (if #t a b) → a
//! - And/Or with constant operands

use std::borrow::Cow;

use sema_core::number::SemaNumber;
use sema_core::{resolve as resolve_spur, Value};

use crate::core_expr::{CoreExpr, PromptEntry};

/// Names that constant folding can fold. If any of these are shadowed
/// by a local binding, folding must be suppressed.
const FOLDABLE_NAMES: &[&str] = &["+", "-", "*", "/", "<", ">", "<=", ">=", "=", "not"];

pub fn optimize(expr: CoreExpr) -> CoreExpr {
    optimize_inner(expr, &Vec::new())
}

fn optimize_inner(expr: CoreExpr, shadowed: &[String]) -> CoreExpr {
    match expr {
        CoreExpr::Call { func, args, tail } => {
            let func = Box::new(optimize_inner(*func, shadowed));
            let args: Vec<_> = args
                .into_iter()
                .map(|a| optimize_inner(a, shadowed))
                .collect();
            try_fold_call(*func, args, tail, shadowed)
        }
        CoreExpr::If { test, then, else_ } => {
            let test = optimize_inner(*test, shadowed);
            let then = optimize_inner(*then, shadowed);
            let else_ = optimize_inner(*else_, shadowed);
            if let CoreExpr::Const(ref v) = test {
                if v.is_truthy() {
                    return then;
                } else {
                    return else_;
                }
            }
            CoreExpr::If {
                test: Box::new(test),
                then: Box::new(then),
                else_: Box::new(else_),
            }
        }
        CoreExpr::And(exprs) => {
            let exprs: Vec<_> = exprs
                .into_iter()
                .map(|e| optimize_inner(e, shadowed))
                .collect();
            fold_and(exprs)
        }
        CoreExpr::Or(exprs) => {
            let exprs: Vec<_> = exprs
                .into_iter()
                .map(|e| optimize_inner(e, shadowed))
                .collect();
            fold_or(exprs)
        }
        CoreExpr::Begin(exprs) => {
            // Scan for define of foldable names — these shadow builtins at top level
            let define_names: Vec<String> = exprs
                .iter()
                .filter_map(|e| {
                    let inner = match e {
                        CoreExpr::Spanned(_, inner) => inner.as_ref(),
                        other => other,
                    };
                    if let CoreExpr::Define(spur, _) = inner {
                        let name = resolve_spur(*spur);
                        if FOLDABLE_NAMES.contains(&name.as_str()) {
                            return Some(name);
                        }
                    }
                    None
                })
                .collect();
            let inner_shadowed = if define_names.is_empty() {
                Cow::Borrowed(shadowed)
            } else {
                extend_shadowed(shadowed, &define_names)
            };
            let exprs: Vec<_> = exprs
                .into_iter()
                .map(|e| optimize_inner(e, &inner_shadowed))
                .collect();
            fold_begin(exprs)
        }
        CoreExpr::Let { bindings, body } => {
            let binding_names: Vec<String> =
                bindings.iter().map(|(s, _)| resolve_spur(*s)).collect();
            let bindings = bindings
                .into_iter()
                .map(|(s, e)| (s, optimize_inner(e, shadowed)))
                .collect();
            let new_shadowed = extend_shadowed(shadowed, &binding_names);
            let body = body
                .into_iter()
                .map(|e| optimize_inner(e, &new_shadowed))
                .collect();
            CoreExpr::Let { bindings, body }
        }
        CoreExpr::LetStar { bindings, body } => {
            let binding_names: Vec<String> =
                bindings.iter().map(|(s, _)| resolve_spur(*s)).collect();
            let new_shadowed = extend_shadowed(shadowed, &binding_names);
            let bindings = bindings
                .into_iter()
                .map(|(s, e)| (s, optimize_inner(e, &new_shadowed)))
                .collect();
            let body = body
                .into_iter()
                .map(|e| optimize_inner(e, &new_shadowed))
                .collect();
            CoreExpr::LetStar { bindings, body }
        }
        CoreExpr::Letrec { bindings, body } => {
            let binding_names: Vec<String> =
                bindings.iter().map(|(s, _)| resolve_spur(*s)).collect();
            let new_shadowed = extend_shadowed(shadowed, &binding_names);
            let bindings = bindings
                .into_iter()
                .map(|(s, e)| (s, optimize_inner(e, &new_shadowed)))
                .collect();
            let body = body
                .into_iter()
                .map(|e| optimize_inner(e, &new_shadowed))
                .collect();
            CoreExpr::Letrec { bindings, body }
        }
        CoreExpr::Lambda(mut def) => {
            let mut param_names: Vec<String> =
                def.params.iter().map(|s| resolve_spur(*s)).collect();
            // The rest param also lexically shadows any builtin of the same name;
            // omitting it let the optimizer constant-fold a shadowed builtin (VM-4).
            if let Some(rest) = def.rest {
                param_names.push(resolve_spur(rest));
            }
            let new_shadowed = extend_shadowed(shadowed, &param_names);
            def.body = def
                .body
                .into_iter()
                .map(|e| optimize_inner(e, &new_shadowed))
                .collect();
            CoreExpr::Lambda(def)
        }
        CoreExpr::Define(spur, expr) => {
            CoreExpr::Define(spur, Box::new(optimize_inner(*expr, shadowed)))
        }
        CoreExpr::Set(spur, expr) => CoreExpr::Set(spur, Box::new(optimize_inner(*expr, shadowed))),
        CoreExpr::Do(mut d) => {
            let var_names: Vec<String> = d.vars.iter().map(|v| resolve_spur(v.name)).collect();
            let new_shadowed = extend_shadowed(shadowed, &var_names);
            d.vars = d
                .vars
                .into_iter()
                .map(|mut v| {
                    v.init = optimize_inner(v.init, shadowed);
                    v.step = v.step.map(|s| optimize_inner(s, &new_shadowed));
                    v
                })
                .collect();
            d.test = Box::new(optimize_inner(*d.test, &new_shadowed));
            d.result = d
                .result
                .into_iter()
                .map(|e| optimize_inner(e, &new_shadowed))
                .collect();
            d.body = d
                .body
                .into_iter()
                .map(|e| optimize_inner(e, &new_shadowed))
                .collect();
            CoreExpr::Do(d)
        }
        CoreExpr::Try {
            body,
            catch_var,
            handler,
        } => {
            let body = body
                .into_iter()
                .map(|e| optimize_inner(e, shadowed))
                .collect();
            let catch_names = vec![resolve_spur(catch_var)];
            let new_shadowed = extend_shadowed(shadowed, &catch_names);
            let handler = handler
                .into_iter()
                .map(|e| optimize_inner(e, &new_shadowed))
                .collect();
            CoreExpr::Try {
                body,
                catch_var,
                handler,
            }
        }
        CoreExpr::Throw(e) => CoreExpr::Throw(Box::new(optimize_inner(*e, shadowed))),
        CoreExpr::MakeList(es) => CoreExpr::MakeList(
            es.into_iter()
                .map(|e| optimize_inner(e, shadowed))
                .collect(),
        ),
        CoreExpr::MakeVector(es) => CoreExpr::MakeVector(
            es.into_iter()
                .map(|e| optimize_inner(e, shadowed))
                .collect(),
        ),
        CoreExpr::MakeMap(pairs) => CoreExpr::MakeMap(
            pairs
                .into_iter()
                .map(|(k, v)| (optimize_inner(k, shadowed), optimize_inner(v, shadowed)))
                .collect(),
        ),
        CoreExpr::Defmacro {
            name,
            params,
            rest,
            body,
        } => {
            let body = body
                .into_iter()
                .map(|e| optimize_inner(e, shadowed))
                .collect();
            CoreExpr::Defmacro {
                name,
                params,
                rest,
                body,
            }
        }
        CoreExpr::Module {
            name,
            exports,
            body,
        } => {
            let body = body
                .into_iter()
                .map(|e| optimize_inner(e, shadowed))
                .collect();
            CoreExpr::Module {
                name,
                exports,
                body,
            }
        }
        CoreExpr::Import { path, selective } => CoreExpr::Import {
            path: Box::new(optimize_inner(*path, shadowed)),
            selective,
        },
        CoreExpr::Load(e) => CoreExpr::Load(Box::new(optimize_inner(*e, shadowed))),
        CoreExpr::Eval(e) => CoreExpr::Eval(Box::new(optimize_inner(*e, shadowed))),
        CoreExpr::Prompt(entries) => CoreExpr::Prompt(
            entries
                .into_iter()
                .map(|entry| match entry {
                    PromptEntry::RoleContent { role, parts } => PromptEntry::RoleContent {
                        role,
                        parts: parts
                            .into_iter()
                            .map(|e| optimize_inner(e, shadowed))
                            .collect(),
                    },
                    PromptEntry::Expr(e) => PromptEntry::Expr(optimize_inner(e, shadowed)),
                })
                .collect(),
        ),
        CoreExpr::Message { role, parts } => CoreExpr::Message {
            role: Box::new(optimize_inner(*role, shadowed)),
            parts: parts
                .into_iter()
                .map(|e| optimize_inner(e, shadowed))
                .collect(),
        },
        CoreExpr::Deftool {
            name,
            description,
            parameters,
            handler,
        } => CoreExpr::Deftool {
            name,
            description: Box::new(optimize_inner(*description, shadowed)),
            parameters: Box::new(optimize_inner(*parameters, shadowed)),
            handler: Box::new(optimize_inner(*handler, shadowed)),
        },
        CoreExpr::Defagent { name, options } => CoreExpr::Defagent {
            name,
            options: Box::new(optimize_inner(*options, shadowed)),
        },
        CoreExpr::Delay(e) => CoreExpr::Delay(Box::new(optimize_inner(*e, shadowed))),
        CoreExpr::Force(e) => CoreExpr::Force(Box::new(optimize_inner(*e, shadowed))),
        CoreExpr::Macroexpand(e) => CoreExpr::Macroexpand(Box::new(optimize_inner(*e, shadowed))),
        CoreExpr::Spanned(span, inner) => {
            CoreExpr::Spanned(span, Box::new(optimize_inner(*inner, shadowed)))
        }
        // Pass through: Const, Var, Quote, DefineRecordType
        other => other,
    }
}

/// Build a new shadowed list, adding only names that are in FOLDABLE_NAMES.
/// Returns a borrowed Cow when no new names are shadowed (the common case),
/// avoiding a Vec allocation on every `let`/`lambda`/`do`/`try` form.
fn extend_shadowed<'a>(current: &'a [String], names: &[String]) -> Cow<'a, [String]> {
    let to_add: Vec<&String> = names
        .iter()
        .filter(|name| FOLDABLE_NAMES.contains(&name.as_str()) && !current.contains(name))
        .collect();
    if to_add.is_empty() {
        return Cow::Borrowed(current);
    }
    let mut result = current.to_vec();
    for name in to_add {
        result.push(name.clone());
    }
    Cow::Owned(result)
}

fn try_fold_call(func: CoreExpr, args: Vec<CoreExpr>, tail: bool, shadowed: &[String]) -> CoreExpr {
    if let CoreExpr::Var(spur) = &func {
        let name = resolve_spur(*spur);
        if !shadowed.iter().any(|s| s == &name) {
            if args.len() == 2 {
                if let (CoreExpr::Const(ref a), CoreExpr::Const(ref b)) = (&args[0], &args[1]) {
                    if let Some(result) = fold_binary_op(&name, a, b) {
                        return CoreExpr::Const(result);
                    }
                }
            } else if args.len() == 1 {
                if let CoreExpr::Const(ref a) = args[0] {
                    if let Some(result) = fold_unary_op(&name, a) {
                        return CoreExpr::Const(result);
                    }
                }
            }
        }
    }
    CoreExpr::Call {
        func: Box::new(func),
        args,
        tail,
    }
}

fn fold_binary_op(name: &str, a: &Value, b: &Value) -> Option<Value> {
    let ai = a.as_int()?;
    let bi = b.as_int()?;
    match name {
        // On overflow, don't fold: leave the call for the runtime, which
        // promotes the result to a bignum rather than silently wrapping.
        "+" => ai.checked_add(bi).map(Value::int),
        "-" => ai.checked_sub(bi).map(Value::int),
        "*" => ai.checked_mul(bi).map(Value::int),
        "/" => {
            if bi == 0 {
                None
            } else if ai.checked_rem(bi) == Some(0) {
                // checked_rem rules out the i64::MIN / -1 overflow pair (it
                // yields None there), so the quotient always fits a fixnum.
                Some(Value::int(ai / bi))
            } else {
                // Not evenly divisible (exact rational), or i64::MIN / -1
                // (quotient 2^63 overflows a fixnum): fold through the tower,
                // matching the runtime `/` (stdlib native fn + `vm_div`)
                // instead of a lossy float — constant folding must not change
                // semantics. A whole-valued rational normalizes to an integer,
                // so the overflow pair folds to a bignum.
                Some(Value::from_number(
                    SemaNumber::from_i64(ai)
                        .div(SemaNumber::from_i64(bi))
                        .unwrap(),
                ))
            }
        }
        "<" => Some(Value::bool(ai < bi)),
        ">" => Some(Value::bool(ai > bi)),
        "<=" => Some(Value::bool(ai <= bi)),
        ">=" => Some(Value::bool(ai >= bi)),
        "=" => Some(Value::bool(ai == bi)),
        _ => None,
    }
}

fn fold_unary_op(name: &str, a: &Value) -> Option<Value> {
    match name {
        "not" => Some(Value::bool(!a.is_truthy())),
        "-" => a.as_int()?.checked_neg().map(Value::int),
        _ => None,
    }
}

fn fold_and(mut exprs: Vec<CoreExpr>) -> CoreExpr {
    while !exprs.is_empty() {
        if let CoreExpr::Const(ref v) = exprs[0] {
            if !v.is_truthy() {
                return exprs.remove(0);
            }
            if exprs.len() > 1 {
                exprs.remove(0);
                continue;
            }
        }
        break;
    }
    if exprs.len() == 1 {
        exprs.pop().unwrap()
    } else {
        CoreExpr::And(exprs)
    }
}

fn fold_or(mut exprs: Vec<CoreExpr>) -> CoreExpr {
    while !exprs.is_empty() {
        if let CoreExpr::Const(ref v) = exprs[0] {
            if v.is_truthy() {
                return exprs.remove(0);
            }
            if exprs.len() > 1 {
                exprs.remove(0);
                continue;
            }
        }
        break;
    }
    if exprs.len() == 1 {
        exprs.pop().unwrap()
    } else {
        CoreExpr::Or(exprs)
    }
}

fn fold_begin(exprs: Vec<CoreExpr>) -> CoreExpr {
    if exprs.len() <= 1 {
        return CoreExpr::Begin(exprs);
    }
    let mut result = Vec::new();
    let last_idx = exprs.len() - 1;
    for (i, e) in exprs.into_iter().enumerate() {
        if i == last_idx || !is_pure_const(&e) {
            result.push(e);
        }
    }
    CoreExpr::Begin(result)
}

fn is_pure_const(e: &CoreExpr) -> bool {
    matches!(e, CoreExpr::Const(_) | CoreExpr::Quote(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lower_str(input: &str) -> CoreExpr {
        let val = sema_reader::read(input).unwrap();
        crate::lower::lower(&val, None).unwrap()
    }

    #[test]
    fn test_shadow_define_in_begin() {
        let core = lower_str("(begin (define + *) (+ 3 4))");
        let optimized = optimize(core);
        // The (+ 3 4) should NOT be folded to 7 because + is redefined
        match &optimized {
            CoreExpr::Begin(exprs) => {
                // The last expression should still be a Call, not a Const
                let last = exprs.last().unwrap();
                assert!(
                    !matches!(last, CoreExpr::Const(_)),
                    "optimizer incorrectly folded (+ 3 4) when + is shadowed by define: {last:?}"
                );
            }
            other => panic!("expected Begin, got {other:?}"),
        }
    }
}
