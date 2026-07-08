use num_bigint::BigInt;
use num_integer::Integer;
use sema_core::number::SemaNumber;
use sema_core::{check_arity, SemaError, Value, ValueViewRef};

use crate::register_fn;

/// Shared "expected number" type error for arithmetic operand validation.
fn not_a_number(arg: &Value) -> SemaError {
    SemaError::type_error("number", arg.type_name())
}

/// `mod`/`modulo`: floored division (result takes the sign of the divisor),
/// per R7RS, over any exact integer (fixnum or bignum). Float operands keep
/// the existing `%` (IEEE truncated remainder) behavior — R7RS `modulo` is an
/// integer-only operation; this stdlib historically also accepted floats, so
/// that path is preserved rather than erroring.
fn mod_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "mod", 2);
    match (args[0].view_ref(), args[1].view_ref()) {
        (ValueViewRef::Float(a), ValueViewRef::Float(b)) => Ok(Value::float(a % b)),
        (ValueViewRef::Int(a), ValueViewRef::Float(b)) => Ok(Value::float(a as f64 % b)),
        (ValueViewRef::Float(a), ValueViewRef::Int(b)) => Ok(Value::float(a % b as f64)),
        _ => {
            let n = args[0]
                .as_bigint()
                .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
            let d = args[1]
                .as_bigint()
                .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?;
            if d == BigInt::from(0) {
                return Err(SemaError::eval("mod: modulo by zero")
                    .with_hint("mod: ensure the divisor is non-zero"));
            }
            Ok(Value::from_bigint(n.mod_floor(&d)))
        }
    }
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "+", |args| {
        if args.is_empty() {
            return Ok(Value::int(0));
        }
        let mut has_float = false;
        let mut int_sum: i64 = 0;
        let mut float_sum: f64 = 0.0;
        // Engaged once an operand overflows the i64 fast path or is itself a
        // bignum; from that point on every remaining operand folds through
        // the tower instead of the i64/f64 accumulators.
        let mut tower: Option<SemaNumber> = None;
        for arg in args {
            if let Some(acc) = tower.take() {
                let n = arg.as_number().ok_or_else(|| not_a_number(arg))?;
                tower = Some(acc.add(n));
                continue;
            }
            match arg.view_ref() {
                ValueViewRef::Int(n) => {
                    if has_float {
                        float_sum += n as f64;
                    } else {
                        match int_sum.checked_add(n) {
                            Some(s) => int_sum = s,
                            None => {
                                tower = Some(
                                    SemaNumber::from_i64(int_sum).add(SemaNumber::from_i64(n)),
                                );
                            }
                        }
                    }
                }
                ValueViewRef::Float(f) => {
                    if !has_float {
                        float_sum = int_sum as f64;
                        has_float = true;
                    }
                    float_sum += f;
                }
                ValueViewRef::BigInt(_) => {
                    let seed = if has_float {
                        SemaNumber::Real(float_sum)
                    } else {
                        SemaNumber::from_i64(int_sum)
                    };
                    let n = arg.as_number().ok_or_else(|| not_a_number(arg))?;
                    tower = Some(seed.add(n));
                }
                _ => return Err(not_a_number(arg)),
            }
        }
        if let Some(acc) = tower {
            Ok(Value::from_number(acc))
        } else if has_float {
            Ok(Value::float(float_sum))
        } else {
            Ok(Value::int(int_sum))
        }
    });

    register_fn(env, "-", |args| {
        check_arity!(args, "-", 1..);

        if args.len() == 1 {
            return match args[0].view_ref() {
                ValueViewRef::Int(n) => match n.checked_neg() {
                    Some(v) => Ok(Value::int(v)),
                    None => Ok(Value::from_number(SemaNumber::from_i64(n).neg())),
                },
                ValueViewRef::Float(f) => Ok(Value::float(-f)),
                ValueViewRef::BigInt(_) => {
                    let n = args[0].as_number().ok_or_else(|| not_a_number(&args[0]))?;
                    Ok(Value::from_number(n.neg()))
                }
                _ => Err(not_a_number(&args[0])),
            };
        }
        let mut has_float = false;
        let mut result_int: i64 = 0;
        let mut result_float: f64 = 0.0;
        // Engaged once an operand overflows the i64 fast path or is itself a
        // bignum; from that point on every remaining operand folds through
        // the tower instead of the i64/f64 accumulators.
        let mut tower: Option<SemaNumber> = None;
        for (i, arg) in args.iter().enumerate() {
            if let Some(acc) = tower.take() {
                let n = arg.as_number().ok_or_else(|| not_a_number(arg))?;
                tower = Some(acc.sub(n));
                continue;
            }
            match arg.view_ref() {
                ValueViewRef::Int(n) => {
                    if i == 0 {
                        if has_float {
                            result_float = n as f64;
                        } else {
                            result_int = n;
                        }
                    } else if has_float {
                        result_float -= n as f64;
                    } else {
                        match result_int.checked_sub(n) {
                            Some(s) => result_int = s,
                            None => {
                                tower = Some(
                                    SemaNumber::from_i64(result_int).sub(SemaNumber::from_i64(n)),
                                );
                            }
                        }
                    }
                }
                ValueViewRef::Float(f) => {
                    if !has_float {
                        result_float = result_int as f64;
                        has_float = true;
                    }
                    if i == 0 {
                        result_float = f;
                    } else {
                        result_float -= f;
                    }
                }
                ValueViewRef::BigInt(_) => {
                    let n = arg.as_number().ok_or_else(|| not_a_number(arg))?;
                    if i == 0 {
                        tower = Some(n);
                    } else {
                        let seed = if has_float {
                            SemaNumber::Real(result_float)
                        } else {
                            SemaNumber::from_i64(result_int)
                        };
                        tower = Some(seed.sub(n));
                    }
                }
                _ => return Err(not_a_number(arg)),
            }
        }
        if let Some(acc) = tower {
            Ok(Value::from_number(acc))
        } else if has_float {
            Ok(Value::float(result_float))
        } else {
            Ok(Value::int(result_int))
        }
    });

    register_fn(env, "*", |args| {
        if args.is_empty() {
            return Ok(Value::int(1));
        }
        let mut has_float = false;
        let mut int_prod: i64 = 1;
        let mut float_prod: f64 = 1.0;
        // Engaged once an operand overflows the i64 fast path or is itself a
        // bignum; from that point on every remaining operand folds through
        // the tower instead of the i64/f64 accumulators.
        let mut tower: Option<SemaNumber> = None;
        for arg in args {
            if let Some(acc) = tower.take() {
                let n = arg.as_number().ok_or_else(|| not_a_number(arg))?;
                tower = Some(acc.mul(n));
                continue;
            }
            match arg.view_ref() {
                ValueViewRef::Int(n) => {
                    if has_float {
                        float_prod *= n as f64;
                    } else {
                        match int_prod.checked_mul(n) {
                            Some(p) => int_prod = p,
                            None => {
                                tower = Some(
                                    SemaNumber::from_i64(int_prod).mul(SemaNumber::from_i64(n)),
                                );
                            }
                        }
                    }
                }
                ValueViewRef::Float(f) => {
                    if !has_float {
                        float_prod = int_prod as f64;
                        has_float = true;
                    }
                    float_prod *= f;
                }
                ValueViewRef::BigInt(_) => {
                    let seed = if has_float {
                        SemaNumber::Real(float_prod)
                    } else {
                        SemaNumber::from_i64(int_prod)
                    };
                    let n = arg.as_number().ok_or_else(|| not_a_number(arg))?;
                    tower = Some(seed.mul(n));
                }
                _ => return Err(not_a_number(arg)),
            }
        }
        if let Some(acc) = tower {
            Ok(Value::from_number(acc))
        } else if has_float {
            Ok(Value::float(float_prod))
        } else {
            Ok(Value::int(int_prod))
        }
    });

    register_fn(env, "/", |args| {
        check_arity!(args, "/", 2..);
        // Fold left through the tower: exact/exact division stays exact
        // (`1/3`, not `0.333…`); any inexact operand contaminates the whole
        // result, matching R7RS exactness contagion.
        let mut acc = args[0].as_number().ok_or_else(|| not_a_number(&args[0]))?;
        for arg in &args[1..] {
            let d = arg.as_number().ok_or_else(|| not_a_number(arg))?;
            acc = acc.div(d).map_err(|_| {
                SemaError::eval("/: division by zero")
                    .with_hint("/: guard with (if (zero? d) ... (/ n d))")
            })?;
        }
        Ok(Value::from_number(acc))
    });

    register_fn(env, "mod", mod_impl);
    register_fn(env, "modulo", mod_impl);
}
