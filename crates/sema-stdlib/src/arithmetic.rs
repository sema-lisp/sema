use num_bigint::BigInt;
use num_integer::Integer;
use sema_core::number::SemaNumber;
use sema_core::{check_arity, SemaError, Value, ValueViewRef};

use crate::register_fn;

/// One fold operator over the numeric tower: the int fast path (returning
/// `None` on overflow so the caller promotes to bignum), the float path, and
/// the bignum/`SemaNumber` path.
fn add_f64(a: f64, b: f64) -> f64 {
    a + b
}
fn sub_f64(a: f64, b: f64) -> f64 {
    a - b
}
fn mul_f64(a: f64, b: f64) -> f64 {
    a * b
}

#[derive(Clone, Copy)]
struct TowerFold {
    int: fn(i64, i64) -> Option<i64>,
    float: fn(f64, f64) -> f64,
    tower: fn(SemaNumber, SemaNumber) -> SemaNumber,
}

const ADD: TowerFold = TowerFold {
    int: i64::checked_add,
    float: add_f64,
    tower: SemaNumber::add,
};
const SUB: TowerFold = TowerFold {
    int: i64::checked_sub,
    float: sub_f64,
    tower: SemaNumber::sub,
};
const MUL: TowerFold = TowerFold {
    int: i64::checked_mul,
    float: mul_f64,
    tower: SemaNumber::mul,
};

/// Fold `values` through the numeric tower: an i64 fast path that promotes to
/// bignum on overflow, and to f64 as soon as any operand is real. The first
/// operand seeds the accumulator by direct assignment (never folded through
/// an identity) so its exact bits, including a signed zero, survive; `fold`
/// combines every operand after that. `identity` is only the result for zero
/// operands (0 for `+`, 1 for `*` — `-` and `*` with 1+ args never reach it).
///
/// Direct-assignment seeding matters because folding through an identity can
/// silently change a signed zero: `0.0 + -0.0 == 0.0` under IEEE 754, so
/// seeding via `0 + first` turned `(- -0.0 0.0)` into `0.0` instead of `-0.0`.
/// `1.0 * -0.0 == -0.0` so `*` was never affected, but `+`'s single-argument
/// case (`(+ -0.0)`) folded through the same `0 + first` and had the same bug.
fn fold_through_tower<'a>(
    values: impl Iterator<Item = &'a Value>,
    identity: i64,
    fold: TowerFold,
) -> Result<Value, SemaError> {
    let mut has_float = false;
    let mut int_acc: i64 = identity;
    let mut float_acc: f64 = identity as f64;
    // Engaged once an operand overflows the i64 fast path or is itself a
    // bignum; from that point on every remaining operand folds through the
    // tower instead of the i64/f64 accumulators.
    let mut tower: Option<SemaNumber> = None;
    for (i, arg) in values.enumerate() {
        let first = i == 0;
        if let Some(acc) = tower.take() {
            let n = arg.as_number().ok_or_else(|| not_a_number(arg))?;
            tower = Some((fold.tower)(acc, n));
            continue;
        }
        match arg.view_ref() {
            ValueViewRef::Int(n) => {
                if has_float {
                    float_acc = (fold.float)(float_acc, n as f64);
                } else if first {
                    int_acc = n;
                } else {
                    match (fold.int)(int_acc, n) {
                        Some(s) => int_acc = s,
                        None => {
                            tower = Some((fold.tower)(
                                SemaNumber::from_i64(int_acc),
                                SemaNumber::from_i64(n),
                            ));
                        }
                    }
                }
            }
            ValueViewRef::Float(f) => {
                if first {
                    float_acc = f;
                    has_float = true;
                } else {
                    if !has_float {
                        float_acc = int_acc as f64;
                        has_float = true;
                    }
                    float_acc = (fold.float)(float_acc, f);
                }
            }
            // Any exact or complex operand beyond the i64/f64 fast paths
            // switches the fold onto the tower for the rest of the operands.
            ValueViewRef::BigInt(_) | ValueViewRef::Rational(_) | ValueViewRef::Complex(_) => {
                let n = arg.as_number().ok_or_else(|| not_a_number(arg))?;
                if first {
                    tower = Some(n);
                } else {
                    let seed_num = if has_float {
                        SemaNumber::Real(float_acc)
                    } else {
                        SemaNumber::from_i64(int_acc)
                    };
                    tower = Some((fold.tower)(seed_num, n));
                }
            }
            _ => return Err(not_a_number(arg)),
        }
    }
    if let Some(acc) = tower {
        Ok(Value::from_number(acc))
    } else if has_float {
        Ok(Value::float(float_acc))
    } else {
        Ok(Value::int(int_acc))
    }
}

/// Sum `values` through the numeric tower (see [`fold_through_tower`]).
/// Shared by `+` and `list/sum` so the two cannot disagree — they did. Where
/// this promotes, `list/sum` used a bare `int_sum += n`, which wrapped silently
/// past i64::MAX, so the same list summed two different ways gave two different
/// answers; it also rejected bignums outright with a self-contradictory
/// "expected number, got int".
pub(crate) fn sum_through_tower<'a>(
    values: impl Iterator<Item = &'a Value>,
) -> Result<Value, SemaError> {
    fold_through_tower(values, 0, ADD)
}

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
    register_fn(env, "+", |args| sum_through_tower(args.iter()));

    register_fn(env, "-", |args| {
        check_arity!(args, "-", 1..);

        if args.len() == 1 {
            return match args[0].view_ref() {
                ValueViewRef::Int(n) => match n.checked_neg() {
                    Some(v) => Ok(Value::int(v)),
                    None => Ok(Value::from_number(SemaNumber::from_i64(n).neg())),
                },
                ValueViewRef::Float(f) => Ok(Value::float(-f)),
                ValueViewRef::BigInt(_) | ValueViewRef::Rational(_) | ValueViewRef::Complex(_) => {
                    let n = args[0].as_number().ok_or_else(|| not_a_number(&args[0]))?;
                    Ok(Value::from_number(n.neg()))
                }
                _ => Err(not_a_number(&args[0])),
            };
        }
        // Seed the accumulator with the first operand directly, then fold the
        // rest with subtraction.
        fold_through_tower(args.iter(), 0, SUB)
    });

    register_fn(env, "*", |args| fold_through_tower(args.iter(), 1, MUL));

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
