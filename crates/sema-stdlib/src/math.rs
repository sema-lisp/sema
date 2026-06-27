use sema_core::{check_arity, SemaError, Value, ValueViewRef};

use crate::register_fn;

fn pow_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "pow", 2);
    match (args[0].view_ref(), args[1].view_ref()) {
        (ValueViewRef::Int(base), ValueViewRef::Int(exp)) if exp >= 0 => {
            Ok(Value::int(base.wrapping_pow(exp as u32)))
        }
        _ => {
            let base = args[0]
                .as_float()
                .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
            let exp = args[1]
                .as_float()
                .ok_or_else(|| SemaError::type_error("number", args[1].type_name()))?;
            Ok(Value::float(base.powf(exp)))
        }
    }
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

fn ceil_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "ceil", 1);
    match args[0].view_ref() {
        ValueViewRef::Int(n) => Ok(Value::int(n)),
        ValueViewRef::Float(f) => float_to_int(f.ceil(), "ceil"),
        _ => Err(SemaError::type_error("number", args[0].type_name())),
    }
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "abs", |args| {
        check_arity!(args, "abs", 1);
        match args[0].view_ref() {
            ValueViewRef::Int(n) => n.checked_abs().map(Value::int).ok_or_else(|| {
                SemaError::eval("abs: |i64::MIN| overflows i64")
                    .with_hint("convert to a float first, e.g. (abs (* 1.0 n))")
            }),
            ValueViewRef::Float(f) => Ok(Value::float(f.abs())),
            _ => Err(SemaError::type_error("number", args[0].type_name())),
        }
    });

    register_fn(env, "min", |args| {
        check_arity!(args, "min", 1..);
        let mut result = args[0].clone();
        for arg in &args[1..] {
            let cmp_result = num_lt(&result, arg)?;
            if !cmp_result {
                result = arg.clone();
            }
        }
        Ok(result)
    });

    register_fn(env, "max", |args| {
        check_arity!(args, "max", 1..);
        let mut result = args[0].clone();
        for arg in &args[1..] {
            let cmp_result = num_lt(arg, &result)?;
            if !cmp_result {
                result = arg.clone();
            }
        }
        Ok(result)
    });

    register_fn(env, "floor", |args| {
        check_arity!(args, "floor", 1);
        match args[0].view_ref() {
            ValueViewRef::Int(n) => Ok(Value::int(n)),
            ValueViewRef::Float(f) => float_to_int(f.floor(), "floor"),
            _ => Err(SemaError::type_error("number", args[0].type_name())),
        }
    });

    register_fn(env, "ceil", ceil_impl);
    register_fn(env, "ceiling", ceil_impl);

    register_fn(env, "round", |args| {
        check_arity!(args, "round", 1);
        match args[0].view_ref() {
            ValueViewRef::Int(n) => Ok(Value::int(n)),
            ValueViewRef::Float(f) => float_to_int(f.round(), "round"),
            _ => Err(SemaError::type_error("number", args[0].type_name())),
        }
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
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.sqrt()))
    });

    register_fn(env, "pow", pow_impl);
    register_fn(env, "expt", pow_impl);
    register_fn(env, "math/pow", pow_impl);

    register_fn(env, "log", |args| {
        check_arity!(args, "log", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.ln()))
    });

    register_fn(env, "sin", |args| {
        check_arity!(args, "sin", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.sin()))
    });

    register_fn(env, "cos", |args| {
        check_arity!(args, "cos", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.cos()))
    });

    // Bind pi and e as constants (bare symbol access)
    env.set_str("pi", Value::float(std::f64::consts::PI));
    env.set_str("e", Value::float(std::f64::consts::E));

    register_fn(env, "int", |args| {
        check_arity!(args, "int", 1);
        match args[0].view_ref() {
            ValueViewRef::Int(n) => Ok(Value::int(n)),
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

    register_fn(env, "math/quotient", |args| {
        check_arity!(args, "math/quotient", 2);
        let a = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
        let b = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        if b == 0 {
            return Err(SemaError::eval("math/quotient: division by zero")
                .with_hint("math/quotient: ensure the divisor is non-zero"));
        }
        Ok(Value::int(a.wrapping_div(b)))
    });

    register_fn(env, "math/remainder", |args| {
        check_arity!(args, "math/remainder", 2);
        let a = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
        let b = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        if b == 0 {
            return Err(SemaError::eval("math/remainder: division by zero")
                .with_hint("math/remainder: ensure the divisor is non-zero"));
        }
        Ok(Value::int(a % b))
    });

    register_fn(env, "math/gcd", |args| {
        check_arity!(args, "math/gcd", 2);
        let mut a = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?
            .wrapping_abs();
        let mut b = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?
            .wrapping_abs();
        while b != 0 {
            let t = b;
            b = a % b;
            a = t;
        }
        Ok(Value::int(a))
    });

    register_fn(env, "math/lcm", |args| {
        check_arity!(args, "math/lcm", 2);
        let a = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?
            .wrapping_abs();
        let b = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?
            .wrapping_abs();
        if a == 0 && b == 0 {
            return Ok(Value::int(0));
        }
        let mut ga = a;
        let mut gb = b;
        while gb != 0 {
            let t = gb;
            gb = ga % gb;
            ga = t;
        }
        Ok(Value::int((a / ga).wrapping_mul(b)))
    });

    register_fn(env, "math/tan", |args| {
        check_arity!(args, "math/tan", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.tan()))
    });

    register_fn(env, "math/asin", |args| {
        check_arity!(args, "math/asin", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.asin()))
    });

    register_fn(env, "math/acos", |args| {
        check_arity!(args, "math/acos", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.acos()))
    });

    register_fn(env, "math/atan", |args| {
        check_arity!(args, "math/atan", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.atan()))
    });

    register_fn(env, "math/atan2", |args| {
        check_arity!(args, "math/atan2", 2);
        let y = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let x = args[1]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[1].type_name()))?;
        Ok(Value::float(y.atan2(x)))
    });

    register_fn(env, "math/exp", |args| {
        check_arity!(args, "math/exp", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.exp()))
    });

    register_fn(env, "math/log10", |args| {
        check_arity!(args, "math/log10", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.log10()))
    });

    register_fn(env, "math/log2", |args| {
        check_arity!(args, "math/log2", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.log2()))
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
        check_arity!(args, "truncate", 1);
        match args[0].view_ref() {
            ValueViewRef::Int(n) => Ok(Value::int(n)),
            ValueViewRef::Float(f) => float_to_int(f.trunc(), "truncate"),
            _ => Err(SemaError::type_error("number", args[0].type_name())),
        }
    });

    register_fn(env, "math/sinh", |args| {
        check_arity!(args, "math/sinh", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.sinh()))
    });

    register_fn(env, "math/cosh", |args| {
        check_arity!(args, "math/cosh", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.cosh()))
    });

    register_fn(env, "math/tanh", |args| {
        check_arity!(args, "math/tanh", 1);
        let f = args[0]
            .as_float()
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        Ok(Value::float(f.tanh()))
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

    env.set_str("math/infinity", Value::float(f64::INFINITY));
    env.set_str("math/nan", Value::float(f64::NAN));
}

fn num_lt(a: &Value, b: &Value) -> Result<bool, SemaError> {
    match (a.view_ref(), b.view_ref()) {
        (ValueViewRef::Int(a), ValueViewRef::Int(b)) => Ok(a < b),
        (ValueViewRef::Float(a), ValueViewRef::Float(b)) => Ok(a < b),
        (ValueViewRef::Int(a), ValueViewRef::Float(b)) => Ok((a as f64) < b),
        (ValueViewRef::Float(a), ValueViewRef::Int(b)) => Ok(a < (b as f64)),
        _ => Err(SemaError::type_error("number", a.type_name())),
    }
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
    fn ceil_impl_guards_nan_and_overflow() {
        // ceil_impl is the only free-fn rounding builtin; exercise it directly.
        assert!(ceil_impl(&[Value::float(f64::NAN)]).is_err());
        assert!(ceil_impl(&[Value::float(1.0e19)]).is_err());
        assert!(ceil_impl(&[Value::float(f64::INFINITY)]).is_err());
        assert_eq!(
            ceil_impl(&[Value::float(2.3)]).unwrap(),
            Value::int(3),
            "ceil(2.3) should still round up to 3"
        );
        // Integer inputs pass through untouched.
        assert_eq!(ceil_impl(&[Value::int(42)]).unwrap(), Value::int(42));
    }

    #[test]
    fn rounding_builtins_error_on_nan_through_env() {
        // Drive the registered closures (floor/round/truncate) end to end so the
        // guard is exercised on every rounding builtin, not just ceil_impl.
        let env = sema_core::Env::new();
        register(&env);
        let ctx = sema_core::EvalContext::default();
        for name in ["ceil", "ceiling", "floor", "round", "truncate"] {
            let f = env.get_str(name).expect("builtin registered");
            let nf = f.as_native_fn_ref().expect("native fn");
            assert!(
                (nf.func)(&ctx, &[Value::float(f64::NAN)]).is_err(),
                "{name} should error on NaN"
            );
            assert!(
                (nf.func)(&ctx, &[Value::float(1.0e19)]).is_err(),
                "{name} should error on out-of-range input"
            );
        }
    }
}
