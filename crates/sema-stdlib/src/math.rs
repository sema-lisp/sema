use std::cmp::Ordering;

use num_bigint::BigInt;
use num_integer::Integer;
use sema_core::number::SemaNumber;
use sema_core::{check_arity, SemaError, Value, ValueViewRef};

use crate::register_fn;

/// `expt`/`pow`/`math/pow`: an exact base (integer/rational) raised to an
/// exact integer exponent stays exact — computed via `SemaNumber::powi`
/// (repeated squaring), so `(expt 2 100)` is a bignum rather than an
/// overflow error and `(expt 2 -3) => 1/8` rather than a float. Any other
/// combination (float base, or a non-integer/float exponent) projects both
/// operands to `f64` and uses `powf`, matching the prior behavior.
fn pow_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "pow", 2);
    let base = args[0]
        .as_number()
        .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
    let exp = args[1]
        .as_number()
        .ok_or_else(|| SemaError::type_error("number", args[1].type_name()))?;
    if base.is_exact() {
        if let SemaNumber::Integer(exp_int) = &exp {
            let result = base
                .powi(exp_int)
                .map_err(|_| SemaError::eval("expt: 0 raised to a negative power"))?;
            return Ok(Value::from_number(result));
        }
    }
    Ok(Value::float(base.to_f64().powf(exp.to_f64())))
}

/// Convert a (rounded) float to an i64, rejecting NaN/infinite values and
/// magnitudes that fall outside the i64 range. A plain `as i64` cast saturates
/// (NaN → 0, out-of-range → i64::MIN/MAX), silently producing garbage, so each
/// rounding builtin guards through this helper instead.
fn float_to_int(f: f64, op: &str) -> Result<Value, SemaError> {
    // i64::MAX is not exactly representable as f64; the smallest f64 strictly
    // greater than i64::MAX is 2^63, so reject anything >= that. i64::MIN
    // (-2^63) is exactly representable, so the lower bound is inclusive.
    const MIN: f64 = i64::MIN as f64;
    const LIMIT: f64 = 9_223_372_036_854_775_808.0; // 2^63
    if !f.is_finite() || !(MIN..LIMIT).contains(&f) {
        return Err(SemaError::eval(format!(
            "{op}: cannot convert {f} to an integer (not finite or out of i64 range)"
        )));
    }
    Ok(Value::int(f as i64))
}

fn to_inexact_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "inexact", 1);
    let n = args[0]
        .as_number()
        .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
    Ok(Value::from_number(n.to_inexact()))
}

fn to_exact_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "exact", 1);
    let n = args[0]
        .as_number()
        .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
    Ok(Value::from_number(n.to_exact()))
}

/// Shared implementation for `floor`/`ceiling`/`round`/`truncate`: lift the
/// argument into the tower, apply the exactness-preserving op, and lower back
/// to the tightest `Value` (exact stays exact, inexact stays inexact — an
/// exact rational rounds to an exact integer, a float rounds to a float).
/// Complex has no rounding and is rejected before `f` ever sees it.
fn round_op(
    args: &[Value],
    name: &str,
    f: impl Fn(SemaNumber) -> SemaNumber,
) -> Result<Value, SemaError> {
    check_arity!(args, name, 1);
    let n = args[0]
        .as_number()
        .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
    if !n.is_real() {
        return Err(SemaError::type_error("real number", args[0].type_name()));
    }
    Ok(Value::from_number(f(n)))
}

fn ceil_impl(args: &[Value]) -> Result<Value, SemaError> {
    round_op(args, "ceil", SemaNumber::ceil)
}

/// Lift a transcendental function's argument to `f64`, accepting any real
/// in the tower (fixnum, bignum, rational, float) — unlike `as_float`, which
/// only handles fixnums and floats and errors on bignum/rational operands.
/// Complex is rejected explicitly rather than silently projecting through
/// `to_f64`'s NaN sentinel.
fn real_arg(v: &Value, name: &str) -> Result<f64, SemaError> {
    let n = v
        .as_number()
        .ok_or_else(|| SemaError::type_error("number", v.type_name()))?;
    if !n.is_real() {
        return Err(SemaError::type_error("real number", v.type_name())
            .with_hint(format!("{name}: complex has no real projection")));
    }
    Ok(n.to_f64())
}

/// `quotient`/`math/quotient`: truncated division (result takes the sign of
/// the dividend), per R7RS. Bignum-aware via `as_bigint`; errors on
/// non-integer operands (rationals, floats, complex).
fn quotient_impl(args: &[Value]) -> Result<Value, SemaError> {
    let (n, d) = two_bigints(args, "quotient")?;
    if d == BigInt::from(0) {
        return Err(SemaError::eval("quotient: division by zero")
            .with_hint("quotient: ensure the divisor is non-zero"));
    }
    Ok(Value::from_bigint(n / d))
}

/// `remainder`/`math/remainder`: truncated-division remainder (result takes
/// the sign of the dividend), per R7RS. Bignum-aware via `as_bigint`.
fn remainder_impl(args: &[Value]) -> Result<Value, SemaError> {
    let (n, d) = two_bigints(args, "remainder")?;
    if d == BigInt::from(0) {
        return Err(SemaError::eval("remainder: division by zero")
            .with_hint("remainder: ensure the divisor is non-zero"));
    }
    Ok(Value::from_bigint(n % d))
}

/// `gcd`/`math/gcd`: variadic fold over bignums. `gcd` of no args is 0 (R7RS),
/// matching the identity of the fold.
fn gcd_impl(args: &[Value]) -> Result<Value, SemaError> {
    let mut acc = BigInt::from(0);
    for arg in args {
        let n = arg
            .as_bigint()
            .ok_or_else(|| SemaError::type_error("integer", arg.type_name()))?;
        acc = acc.gcd(&n);
    }
    Ok(Value::from_bigint(acc))
}

/// `lcm`/`math/lcm`: variadic fold over bignums. `lcm` of no args is 1
/// (R7RS), matching the identity of the fold.
fn lcm_impl(args: &[Value]) -> Result<Value, SemaError> {
    let mut acc = BigInt::from(1);
    for arg in args {
        let n = arg
            .as_bigint()
            .ok_or_else(|| SemaError::type_error("integer", arg.type_name()))?;
        acc = acc.lcm(&n);
    }
    Ok(Value::from_bigint(acc))
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "abs", |args| {
        check_arity!(args, "abs", 1);
        match args[0].view_ref() {
            // Fixnum fast path; `|i64::MIN|` overflows i64, so fall through to
            // the tower where it promotes to an exact bignum.
            ValueViewRef::Int(n) => match n.checked_abs() {
                Some(a) => Ok(Value::int(a)),
                None => {
                    let m = args[0]
                        .as_number()
                        .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
                    Ok(Value::from_number(m.abs()))
                }
            },
            ValueViewRef::Float(f) => Ok(Value::float(f.abs())),
            // Bignum/rational/complex: `SemaNumber::abs` is exactness-preserving
            // for reals (negate iff negative) and projects a complex to its
            // (inexact) magnitude.
            _ => {
                let n = args[0]
                    .as_number()
                    .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
                Ok(Value::from_number(n.abs()))
            }
        }
    });

    register_fn(env, "min", |args| {
        min_max_fold(args, "min", |ord| {
            matches!(ord, Ordering::Less | Ordering::Equal)
        })
    });

    register_fn(env, "max", |args| {
        min_max_fold(args, "max", |ord| {
            matches!(ord, Ordering::Greater | Ordering::Equal)
        })
    });

    register_fn(env, "floor", |args| {
        round_op(args, "floor", SemaNumber::floor)
    });

    register_fn(env, "ceil", ceil_impl);
    register_fn(env, "ceiling", ceil_impl);

    // R7RS "banker's rounding": ties round to the nearest even integer.
    register_fn(env, "round", |args| {
        round_op(args, "round", SemaNumber::round)
    });

    // Round to `places` decimal places, returning a float: (math/round-to 3.14159 2) => 3.14.
    register_fn(env, "math/round-to", |args| {
        check_arity!(args, "math/round-to", 2);
        let x = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let places = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?;
        let factor = 10f64.powi(places as i32);
        Ok(Value::float((x * factor).round() / factor))
    });

    // Fixed-decimal display string, padding trailing zeros: (math/format-fixed 1.2 3) => "1.200".
    register_fn(env, "math/format-fixed", |args| {
        check_arity!(args, "math/format-fixed", 2);
        let x = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let places = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?
            .max(0) as usize;
        Ok(Value::string(&format!("{x:.places$}")))
    });

    register_fn(env, "sqrt", |args| {
        check_arity!(args, "sqrt", 1);
        let n = args[0]
            .as_number()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        match n {
            SemaNumber::Complex(c) => {
                // Principal complex square root via polar form:
                // sqrt(r·e^iθ) = sqrt(r)·e^(iθ/2).
                let (re, im) = (c.re.to_f64(), c.im.to_f64());
                let r = re.hypot(im).sqrt();
                let theta = im.atan2(re) / 2.0;
                Ok(Value::complex(
                    SemaNumber::Real(r * theta.cos()),
                    SemaNumber::Real(r * theta.sin()),
                ))
            }
            real if real.cmp_real(&SemaNumber::from_i64(0)) == Some(std::cmp::Ordering::Less) => {
                // A negative real has no real square root: sqrt(-x) = sqrt(x)·i.
                // An exact perfect square keeps an exact imaginary part
                // (R7RS: (sqrt -1) => +i, (sqrt -4) => +2i).
                let mag = real.clone().abs();
                let im = mag
                    .exact_sqrt()
                    .unwrap_or_else(|| SemaNumber::Real(mag.to_f64().sqrt()));
                Ok(Value::complex(SemaNumber::from_i64(0), im))
            }
            // An exact perfect square stays exact: (sqrt 4) => 2.
            real => match real.exact_sqrt() {
                Some(root) => Ok(Value::from_number(root)),
                None => Ok(Value::float(real.to_f64().sqrt())),
            },
        }
    });

    register_fn(env, "make-rectangular", |args| {
        check_arity!(args, "make-rectangular", 2);
        let re = args[0]
            .as_number()
            .filter(|n| n.is_real())
            .ok_or_else(|| SemaError::type_error("real", args[0].type_name()))?;
        let im = args[1]
            .as_number()
            .filter(|n| n.is_real())
            .ok_or_else(|| SemaError::type_error("real", args[1].type_name()))?;
        Ok(Value::complex(re, im))
    });

    register_fn(env, "make-polar", |args| {
        check_arity!(args, "make-polar", 2);
        let m = args[0]
            .as_number()
            .filter(|n| n.is_real())
            .map(|n| n.to_f64())
            .ok_or_else(|| SemaError::type_error("real", args[0].type_name()))?;
        let a = args[1]
            .as_number()
            .filter(|n| n.is_real())
            .map(|n| n.to_f64())
            .ok_or_else(|| SemaError::type_error("real", args[1].type_name()))?;
        Ok(Value::complex(
            SemaNumber::Real(m * a.cos()),
            SemaNumber::Real(m * a.sin()),
        ))
    });

    register_fn(env, "real-part", |args| {
        check_arity!(args, "real-part", 1);
        match args[0].as_number() {
            Some(SemaNumber::Complex(c)) => Ok(Value::from_number(c.re)),
            Some(real) => Ok(Value::from_number(real)),
            None => Err(SemaError::type_error("number", args[0].type_name())),
        }
    });

    register_fn(env, "imag-part", |args| {
        check_arity!(args, "imag-part", 1);
        match args[0].as_number() {
            Some(SemaNumber::Complex(c)) => Ok(Value::from_number(c.im)),
            // A real number has an exact 0 imaginary part per R7RS.
            Some(_) => Ok(Value::int(0)),
            None => Err(SemaError::type_error("number", args[0].type_name())),
        }
    });

    register_fn(env, "magnitude", |args| {
        check_arity!(args, "magnitude", 1);
        match args[0].as_number() {
            // `abs` already covers every tower level: exact for real inputs,
            // the (inexact) hypot for `Complex`.
            Some(n) => Ok(Value::from_number(n.abs())),
            None => Err(SemaError::type_error("number", args[0].type_name())),
        }
    });

    register_fn(env, "angle", |args| {
        check_arity!(args, "angle", 1);
        match args[0].as_number() {
            Some(SemaNumber::Complex(c)) => Ok(Value::float(c.im.to_f64().atan2(c.re.to_f64()))),
            Some(real) => Ok(Value::float(if real.to_f64() < 0.0 {
                std::f64::consts::PI
            } else {
                0.0
            })),
            None => Err(SemaError::type_error("number", args[0].type_name())),
        }
    });

    // Exactness conversions (R7RS). `exact`/`inexact->exact` snap a finite real
    // to its exact rational value; `inexact`/`exact->inexact` project every
    // component to a float. Both lower back to the tightest representation.
    register_fn(env, "inexact", to_inexact_impl);
    register_fn(env, "exact->inexact", to_inexact_impl);
    register_fn(env, "exact", to_exact_impl);
    register_fn(env, "inexact->exact", to_exact_impl);

    register_fn(env, "pow", pow_impl);
    register_fn(env, "expt", pow_impl);
    register_fn(env, "math/pow", pow_impl);

    register_fn(env, "log", |args| {
        check_arity!(args, "log", 1);
        Ok(Value::float(real_arg(&args[0], "log")?.ln()))
    });

    register_fn(env, "sin", |args| {
        check_arity!(args, "sin", 1);
        Ok(Value::float(real_arg(&args[0], "sin")?.sin()))
    });

    register_fn(env, "cos", |args| {
        check_arity!(args, "cos", 1);
        Ok(Value::float(real_arg(&args[0], "cos")?.cos()))
    });

    // Bind pi and e as constants (bare symbol access)
    env.set_str("pi", Value::float(std::f64::consts::PI));
    env.set_str("e", Value::float(std::f64::consts::E));

    register_fn(env, "int", |args| {
        check_arity!(args, "int", 1);
        match args[0].view_ref() {
            ValueViewRef::Int(n) => Ok(Value::int(n)),
            // Already an exact integer — identity, stays a bignum.
            ValueViewRef::BigInt(b) => Ok(Value::from_bigint(b.clone())),
            // An exact rational truncates toward zero to an exact integer.
            ValueViewRef::Rational(r) => Ok(Value::from_bigint(r.trunc().to_integer())),
            // Truncate toward zero, but reject NaN/inf/out-of-range like every
            // other rounding builtin (floor/ceil/round/truncate) — a raw cast
            // would saturate and silently return garbage.
            ValueViewRef::Float(f) => float_to_int(f.trunc(), "int"),
            ValueViewRef::String(s) => s
                .parse::<i64>()
                .map(Value::int)
                .map_err(|_| SemaError::eval(format!("cannot convert '{s}' to int"))),
            _ => Err(SemaError::type_error(
                "number or string",
                args[0].type_name(),
            )),
        }
    });

    register_fn(env, "float", |args| {
        check_arity!(args, "float", 1);
        match args[0].view_ref() {
            ValueViewRef::Int(n) => Ok(Value::float(n as f64)),
            ValueViewRef::Float(f) => Ok(Value::float(f)),
            // Bignums and rationals project inexactly (like exact->inexact);
            // real_arg rejects complex, which has no real projection.
            ValueViewRef::BigInt(_) | ValueViewRef::Rational(_) | ValueViewRef::Complex(_) => {
                Ok(Value::float(real_arg(&args[0], "float")?))
            }
            ValueViewRef::String(s) => s
                .parse::<f64>()
                .map(Value::float)
                .map_err(|_| SemaError::eval(format!("cannot convert '{s}' to float"))),
            _ => Err(SemaError::type_error(
                "number or string",
                args[0].type_name(),
            )),
        }
    });

    register_fn(env, "math/tan", |args| {
        check_arity!(args, "math/tan", 1);
        Ok(Value::float(real_arg(&args[0], "math/tan")?.tan()))
    });

    register_fn(env, "math/asin", |args| {
        check_arity!(args, "math/asin", 1);
        Ok(Value::float(real_arg(&args[0], "math/asin")?.asin()))
    });

    register_fn(env, "math/acos", |args| {
        check_arity!(args, "math/acos", 1);
        Ok(Value::float(real_arg(&args[0], "math/acos")?.acos()))
    });

    register_fn(env, "math/atan", |args| {
        check_arity!(args, "math/atan", 1);
        Ok(Value::float(real_arg(&args[0], "math/atan")?.atan()))
    });

    register_fn(env, "math/atan2", |args| {
        check_arity!(args, "math/atan2", 2);
        let y = real_arg(&args[0], "math/atan2")?;
        let x = real_arg(&args[1], "math/atan2")?;
        Ok(Value::float(y.atan2(x)))
    });

    register_fn(env, "math/exp", |args| {
        check_arity!(args, "math/exp", 1);
        Ok(Value::float(real_arg(&args[0], "math/exp")?.exp()))
    });

    register_fn(env, "math/log10", |args| {
        check_arity!(args, "math/log10", 1);
        Ok(Value::float(real_arg(&args[0], "math/log10")?.log10()))
    });

    register_fn(env, "math/log2", |args| {
        check_arity!(args, "math/log2", 1);
        Ok(Value::float(real_arg(&args[0], "math/log2")?.log2()))
    });

    register_fn(env, "math/random", |args| {
        check_arity!(args, "math/random", 0);
        Ok(Value::float(rand::random::<f64>()))
    });

    register_fn(env, "math/random-int", |args| {
        check_arity!(args, "math/random-int", 2);
        let lo = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
        let hi = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        if lo > hi {
            return Err(SemaError::eval(format!(
                "math/random-int: lo ({lo}) must be <= hi ({hi})"
            )));
        }
        use rand::RngExt;
        let val = rand::rng().random_range(lo..=hi);
        Ok(Value::int(val))
    });

    register_fn(env, "math/clamp", |args| {
        check_arity!(args, "math/clamp", 3);
        match (args[0].view_ref(), args[1].view_ref(), args[2].view_ref()) {
            (ValueViewRef::Int(v), ValueViewRef::Int(lo), ValueViewRef::Int(hi)) => {
                Ok(Value::int(v.max(lo).min(hi)))
            }
            _ => {
                let v = args[0]
                    .as_float()
                    .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
                let lo = args[1]
                    .as_float()
                    .ok_or_else(|| SemaError::type_error("number", args[1].type_name()))?;
                let hi = args[2]
                    .as_float()
                    .ok_or_else(|| SemaError::type_error("number", args[2].type_name()))?;
                // f64::max/min discard NaN, so a NaN input would silently become
                // a bound. Propagate NaN instead, matching IEEE-754 expectations.
                if v.is_nan() {
                    return Ok(Value::float(v));
                }
                Ok(Value::float(v.max(lo).min(hi)))
            }
        }
    });

    register_fn(env, "math/sign", |args| {
        check_arity!(args, "math/sign", 1);
        match args[0].view_ref() {
            ValueViewRef::Int(n) => Ok(Value::int(if n > 0 {
                1
            } else if n < 0 {
                -1
            } else {
                0
            })),
            ValueViewRef::Float(f) => Ok(Value::int(if f > 0.0 {
                1
            } else if f < 0.0 {
                -1
            } else {
                0
            })),
            _ => Err(SemaError::type_error("number", args[0].type_name())),
        }
    });

    register_fn(env, "truncate", |args| {
        round_op(args, "truncate", SemaNumber::truncate)
    });

    register_fn(env, "math/sinh", |args| {
        check_arity!(args, "math/sinh", 1);
        Ok(Value::float(real_arg(&args[0], "math/sinh")?.sinh()))
    });

    register_fn(env, "math/cosh", |args| {
        check_arity!(args, "math/cosh", 1);
        Ok(Value::float(real_arg(&args[0], "math/cosh")?.cosh()))
    });

    register_fn(env, "math/tanh", |args| {
        check_arity!(args, "math/tanh", 1);
        Ok(Value::float(real_arg(&args[0], "math/tanh")?.tanh()))
    });

    register_fn(env, "math/degrees->radians", |args| {
        check_arity!(args, "math/degrees->radians", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.to_radians()))
    });

    register_fn(env, "math/radians->degrees", |args| {
        check_arity!(args, "math/radians->degrees", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.to_degrees()))
    });

    register_fn(env, "math/lerp", |args| {
        check_arity!(args, "math/lerp", 3);
        let a = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let b = args[1]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[1].type_name()))?;
        let t = args[2]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[2].type_name()))?;
        Ok(Value::float(a + (b - a) * t))
    });

    register_fn(env, "math/map-range", |args| {
        check_arity!(args, "math/map-range", 5);
        let value = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let in_min = args[1]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[1].type_name()))?;
        let in_max = args[2]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[2].type_name()))?;
        let out_min = args[3]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[3].type_name()))?;
        let out_max = args[4]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[4].type_name()))?;
        Ok(Value::float(
            out_min + (value - in_min) / (in_max - in_min) * (out_max - out_min),
        ))
    });

    register_fn(env, "math/nan?", |args| {
        check_arity!(args, "math/nan?", 1);
        match args[0].view_ref() {
            ValueViewRef::Float(f) => Ok(Value::bool(f.is_nan())),
            _ => Ok(Value::bool(false)),
        }
    });

    register_fn(env, "math/infinite?", |args| {
        check_arity!(args, "math/infinite?", 1);
        match args[0].view_ref() {
            ValueViewRef::Float(f) => Ok(Value::bool(f.is_infinite())),
            _ => Ok(Value::bool(false)),
        }
    });

    register_fn(env, "numerator", |args| {
        check_arity!(args, "numerator", 1);
        match args[0].as_rational() {
            Some(r) => Ok(Value::from_bigint(r.numer().clone())),
            None => Err(SemaError::type_error("rational", args[0].type_name())),
        }
    });

    register_fn(env, "denominator", |args| {
        check_arity!(args, "denominator", 1);
        match args[0].as_rational() {
            Some(r) => Ok(Value::from_bigint(r.denom().clone())),
            None => Err(SemaError::type_error("rational", args[0].type_name())),
        }
    });

    // quotient/remainder/gcd/lcm: shared bignum-aware implementations,
    // registered under both the unprefixed R7RS name and the `math/` alias
    // (same pattern as `pow`/`expt`/`math/pow`).
    register_fn(env, "quotient", quotient_impl);
    register_fn(env, "math/quotient", quotient_impl);
    register_fn(env, "remainder", remainder_impl);
    register_fn(env, "math/remainder", remainder_impl);
    register_fn(env, "gcd", gcd_impl);
    register_fn(env, "math/gcd", gcd_impl);
    register_fn(env, "lcm", lcm_impl);
    register_fn(env, "math/lcm", lcm_impl);

    // `(exact-integer-sqrt n)` → the list `(s r)` where `s = ⌊√n⌋` and
    // `r = n - s²`, so `s² + r = n` and `0 ≤ r ≤ 2s`. Requires a non-negative
    // exact integer (fixnum or bignum); rejects rationals, floats, and complex.
    register_fn(env, "exact-integer-sqrt", |args| {
        check_arity!(args, "exact-integer-sqrt", 1);
        let n = args[0].as_bigint().ok_or_else(|| {
            SemaError::type_error("exact integer", args[0].type_name())
                .with_hint("exact-integer-sqrt: argument must be an exact integer")
        })?;
        if n.sign() == num_bigint::Sign::Minus {
            return Err(
                SemaError::eval("exact-integer-sqrt: argument must be non-negative")
                    .with_hint("exact-integer-sqrt: √ of a negative integer is not real"),
            );
        }
        let s = n.sqrt();
        let r = &n - &s * &s;
        Ok(Value::list(vec![
            Value::from_bigint(s),
            Value::from_bigint(r),
        ]))
    });

    // `(rationalize x tol)` → the simplest rational within `tol` of `x` (R7RS).
    // Both arguments must be real. Exactness follows the tower's contagion rule
    // (inexact iff either operand is inexact); the math is the Stern–Brocot
    // "simplest rational in interval" descent in `SemaNumber::rationalize`.
    register_fn(env, "rationalize", |args| {
        check_arity!(args, "rationalize", 2);
        let x = args[0]
            .as_number()
            .ok_or_else(|| SemaError::type_error("real", args[0].type_name()))?;
        let tol = args[1]
            .as_number()
            .ok_or_else(|| SemaError::type_error("real", args[1].type_name()))?;
        if !x.is_real() || !tol.is_real() {
            return Err(SemaError::type_error(
                "real",
                args[if x.is_real() { 1 } else { 0 }].type_name(),
            )
            .with_hint("rationalize: complex numbers have no simplest-rational approximation"));
        }
        Ok(Value::from_number(x.rationalize(&tol)))
    });

    env.set_str("math/infinity", Value::float(f64::INFINITY));
    env.set_str("math/nan", Value::float(f64::NAN));
}

/// Lift both arguments of a 2-arg integer builtin (`quotient`/`remainder`) to
/// `BigInt`, erroring on non-integer operands (rationals, floats, complex).
fn two_bigints(args: &[Value], name: &str) -> Result<(BigInt, BigInt), SemaError> {
    check_arity!(args, name, 2);
    let n = args[0]
        .as_bigint()
        .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
    let d = args[1]
        .as_bigint()
        .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?;
    Ok((n, d))
}

/// Shared fold for `min`/`max` over the whole tower. `keep_first` decides,
/// given the ordering of the running extremum relative to the next
/// candidate, whether to keep the extremum (`true`) or replace it. Applies
/// R7RS inexactness contagion: if *any* argument was inexact, the winner is
/// converted to inexact at the end, even if the winning value itself was
/// exact.
fn min_max_fold(
    args: &[Value],
    name: &str,
    keep_first: impl Fn(Ordering) -> bool,
) -> Result<Value, SemaError> {
    check_arity!(args, name, 1..);
    let mut best = args[0]
        .as_number()
        .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
    let mut any_inexact = !best.is_exact();
    for arg in &args[1..] {
        let n = arg
            .as_number()
            .ok_or_else(|| SemaError::type_error("number", arg.type_name()))?;
        any_inexact = any_inexact || !n.is_exact();
        let ord = best.cmp_real(&n).ok_or_else(|| {
            SemaError::eval(format!("{name}: cannot order these numbers"))
                .with_hint("complex numbers (and NaN) have no ordering")
        })?;
        if !keep_first(ord) {
            best = n;
        }
    }
    if any_inexact {
        best = best.to_inexact();
    }
    Ok(Value::from_number(best))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_to_int_rejects_non_finite_and_out_of_range() {
        // NaN and infinities must error, not silently become 0 / saturate.
        assert!(float_to_int(f64::NAN, "round").is_err());
        assert!(float_to_int(f64::INFINITY, "ceil").is_err());
        assert!(float_to_int(f64::NEG_INFINITY, "floor").is_err());

        // Magnitudes beyond i64 range must error rather than saturate.
        assert!(float_to_int(1.0e19, "round").is_err());
        assert!(float_to_int(-1.0e19, "round").is_err());
        // Exactly 2^63 is out of range (i64::MAX is 2^63 - 1).
        assert!(float_to_int(9_223_372_036_854_775_808.0, "round").is_err());
    }

    #[test]
    fn float_to_int_accepts_in_range_values() {
        assert_eq!(float_to_int(3.0, "ceil").unwrap(), Value::int(3));
        assert_eq!(float_to_int(-3.0, "floor").unwrap(), Value::int(-3));
        assert_eq!(float_to_int(0.0, "round").unwrap(), Value::int(0));
        // i64::MIN is exactly representable as f64 and must be accepted.
        assert_eq!(
            float_to_int(i64::MIN as f64, "trunc").unwrap(),
            Value::int(i64::MIN)
        );
    }

    #[test]
    fn ceil_impl_preserves_exactness() {
        // Integer inputs pass through untouched (still exact).
        assert_eq!(ceil_impl(&[Value::int(42)]).unwrap(), Value::int(42));
        // A float ceils to a float — inexact stays inexact, unlike the old
        // behavior that lossily converted the result to an int.
        assert_eq!(ceil_impl(&[Value::float(2.3)]).unwrap(), Value::float(3.0));
        // Complex has no rounding.
        assert!(ceil_impl(&[Value::complex(
            SemaNumber::from_i64(1),
            SemaNumber::from_i64(1)
        )])
        .is_err());
    }

    #[test]
    fn rounding_builtins_preserve_float_exactness_through_env() {
        // Drive the registered closures (floor/round/truncate/ceil) end to end.
        // Since these no longer convert their result to an i64, NaN/out-of-range
        // floats are no longer an overflow risk — they pass straight through.
        let env = sema_core::Env::new();
        register(&env);
        let ctx = sema_core::EvalContext::default();
        for name in ["ceil", "ceiling", "floor", "round", "truncate"] {
            let f = env.get_str(name).expect("builtin registered");
            let nf = f.as_native_fn_ref().expect("native fn");
            let nan_result = (nf.func)(&ctx, &[Value::float(f64::NAN)]).unwrap();
            assert!(
                matches!(nan_result.as_float(), Some(f) if f.is_nan()),
                "{name} should return NaN unchanged"
            );
            let big_result = (nf.func)(&ctx, &[Value::float(1.0e19)]).unwrap();
            assert_eq!(
                big_result,
                Value::float(1.0e19),
                "{name} should return large floats unchanged"
            );
        }
    }
}
