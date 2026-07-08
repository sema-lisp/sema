use num_bigint::BigInt;

use sema_core::{check_arity, SemaError, Value};

use crate::register_fn;

/// Lift an operand to `BigInt`, erroring with the builtin's name on
/// non-integers (matches the `as_int` error shape these ops had before).
fn require_bigint(v: &Value, name: &str) -> Result<BigInt, SemaError> {
    v.as_bigint()
        .ok_or_else(|| SemaError::type_error("int", v.type_name()).with_hint(name.to_string()))
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "bit/and", |args| {
        check_arity!(args, "bit/and", 2);
        let a = require_bigint(&args[0], "bit/and")?;
        let b = require_bigint(&args[1], "bit/and")?;
        Ok(Value::from_bigint(a & b))
    });

    register_fn(env, "bit/or", |args| {
        check_arity!(args, "bit/or", 2);
        let a = require_bigint(&args[0], "bit/or")?;
        let b = require_bigint(&args[1], "bit/or")?;
        Ok(Value::from_bigint(a | b))
    });

    register_fn(env, "bit/xor", |args| {
        check_arity!(args, "bit/xor", 2);
        let a = require_bigint(&args[0], "bit/xor")?;
        let b = require_bigint(&args[1], "bit/xor")?;
        Ok(Value::from_bigint(a ^ b))
    });

    register_fn(env, "bit/not", |args| {
        check_arity!(args, "bit/not", 1);
        let a = require_bigint(&args[0], "bit/not")?;
        Ok(Value::from_bigint(!a))
    });

    register_fn(env, "bit/shift-left", |args| {
        check_arity!(args, "bit/shift-left", 2);
        let a = require_bigint(&args[0], "bit/shift-left")?;
        let n = args[1].as_index("bit/shift-left")?;
        Ok(Value::from_bigint(a << n))
    });

    register_fn(env, "bit/shift-right", |args| {
        check_arity!(args, "bit/shift-right", 2);
        let a = require_bigint(&args[0], "bit/shift-right")?;
        let n = args[1].as_index("bit/shift-right")?;
        Ok(Value::from_bigint(a >> n))
    });
}
