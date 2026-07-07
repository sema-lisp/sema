use std::cmp::Ordering;

use num_integer::Integer;
use sema_core::number::SemaNumber;
use sema_core::{check_arity, SemaError, Value};

use crate::register_fn;

/// Exact numeric ordering across the whole tower (int/bignum/rational/float),
/// or `None` for an unordered NaN. Errors if either argument is not a number,
/// or if either is complex (complex numbers have no ordering — only `=`/
/// `zero?` are meaningful for them).
fn num_partial_cmp(a: &Value, b: &Value) -> Result<Option<Ordering>, SemaError> {
    let na = a
        .as_number()
        .ok_or_else(|| SemaError::type_error("number", a.type_name()))?;
    let nb = b
        .as_number()
        .ok_or_else(|| SemaError::type_error("number", b.type_name()))?;
    if !na.is_real() || !nb.is_real() {
        return Err(SemaError::eval("cannot order complex numbers")
            .with_hint("complex numbers have no ordering; use = or zero? instead"));
    }
    Ok(na.cmp_real(&nb))
}

fn num_cmp(
    args: &[Value],
    op: &str,
    want: impl Fn(Option<Ordering>) -> bool,
) -> Result<Value, SemaError> {
    check_arity!(args, op, 2..);
    for pair in args.windows(2) {
        if !want(num_partial_cmp(&pair[0], &pair[1])?) {
            return Ok(Value::bool(false));
        }
    }
    Ok(Value::bool(true))
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "<", |args| {
        num_cmp(args, "<", |o| o == Some(Ordering::Less))
    });
    register_fn(env, ">", |args| {
        num_cmp(args, ">", |o| o == Some(Ordering::Greater))
    });
    register_fn(env, "<=", |args| {
        num_cmp(args, "<=", |o| {
            matches!(o, Some(Ordering::Less | Ordering::Equal))
        })
    });
    register_fn(env, ">=", |args| {
        num_cmp(args, ">=", |o| {
            matches!(o, Some(Ordering::Greater | Ordering::Equal))
        })
    });

    register_fn(env, "=", |args| {
        check_arity!(args, "=", 2..);
        for pair in args.windows(2) {
            match (pair[0].as_number(), pair[1].as_number()) {
                (Some(a), Some(b)) => {
                    if !a.num_eq(&b) {
                        return Ok(Value::bool(false));
                    }
                }
                (None, _) => return Err(SemaError::type_error("number", pair[0].type_name())),
                (_, None) => return Err(SemaError::type_error("number", pair[1].type_name())),
            }
        }
        Ok(Value::bool(true))
    });

    register_fn(env, "eq?", |args| {
        check_arity!(args, "eq?", 2);
        Ok(Value::bool(args[0] == args[1]))
    });

    register_fn(env, "not", |args| {
        check_arity!(args, "not", 1);
        Ok(Value::bool(!args[0].is_truthy()))
    });

    register_fn(env, "zero?", |args| {
        check_arity!(args, "zero?", 1);
        let n = args[0]
            .as_number()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::bool(
            n.cmp_real(&SemaNumber::from_i64(0)) == Some(Ordering::Equal),
        ))
    });

    register_fn(env, "positive?", |args| {
        check_arity!(args, "positive?", 1);
        let n = args[0]
            .as_number()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::bool(
            n.cmp_real(&SemaNumber::from_i64(0)) == Some(Ordering::Greater),
        ))
    });

    register_fn(env, "negative?", |args| {
        check_arity!(args, "negative?", 1);
        let n = args[0]
            .as_number()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::bool(
            n.cmp_real(&SemaNumber::from_i64(0)) == Some(Ordering::Less),
        ))
    });

    register_fn(env, "even?", |args| {
        check_arity!(args, "even?", 1);
        let n = args[0]
            .as_bigint()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
        Ok(Value::bool(n.is_even()))
    });

    register_fn(env, "odd?", |args| {
        check_arity!(args, "odd?", 1);
        let n = args[0]
            .as_bigint()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
        Ok(Value::bool(n.is_odd()))
    });
}
