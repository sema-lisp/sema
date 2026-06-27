use sema_core::{check_arity, SemaError, Value, ValueViewRef};

use crate::register_fn;

fn mod_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "mod", 2);
    match (args[0].view_ref(), args[1].view_ref()) {
        (ValueViewRef::Int(a), ValueViewRef::Int(b)) => {
            if b == 0 {
                Err(SemaError::eval("mod: modulo by zero")
                    .with_hint("mod: ensure the divisor is non-zero"))
            } else {
                Ok(Value::int(a % b))
            }
        }
        (ValueViewRef::Float(a), ValueViewRef::Float(b)) => Ok(Value::float(a % b)),
        (ValueViewRef::Int(a), ValueViewRef::Float(b)) => Ok(Value::float(a as f64 % b)),
        (ValueViewRef::Float(a), ValueViewRef::Int(b)) => Ok(Value::float(a % b as f64)),
        _ => Err(SemaError::type_error("number", args[0].type_name())),
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
        for arg in args {
            match arg.view_ref() {
                ValueViewRef::Int(n) => {
                    if has_float {
                        float_sum += n as f64;
                    } else {
                        int_sum = int_sum.wrapping_add(n);
                    }
                }
                ValueViewRef::Float(f) => {
                    if !has_float {
                        float_sum = int_sum as f64;
                        has_float = true;
                    }
                    float_sum += f;
                }
                _ => return Err(SemaError::type_error("number", arg.type_name())),
            }
        }
        if has_float {
            Ok(Value::float(float_sum))
        } else {
            Ok(Value::int(int_sum))
        }
    });

    register_fn(env, "-", |args| {
        check_arity!(args, "-", 1..);

        if args.len() == 1 {
            return match args[0].view_ref() {
                ValueViewRef::Int(n) => Ok(Value::int(n.wrapping_neg())),
                ValueViewRef::Float(f) => Ok(Value::float(-f)),
                _ => Err(SemaError::type_error("number", args[0].type_name())),
            };
        }
        let mut has_float = false;
        let mut result_int: i64 = 0;
        let mut result_float: f64 = 0.0;
        for (i, arg) in args.iter().enumerate() {
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
                        result_int = result_int.wrapping_sub(n);
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
                _ => return Err(SemaError::type_error("number", arg.type_name())),
            }
        }
        if has_float {
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
        for arg in args {
            match arg.view_ref() {
                ValueViewRef::Int(n) => {
                    if has_float {
                        float_prod *= n as f64;
                    } else {
                        int_prod = int_prod.wrapping_mul(n);
                    }
                }
                ValueViewRef::Float(f) => {
                    if !has_float {
                        float_prod = int_prod as f64;
                        has_float = true;
                    }
                    float_prod *= f;
                }
                _ => return Err(SemaError::type_error("number", arg.type_name())),
            }
        }
        if has_float {
            Ok(Value::float(float_prod))
        } else {
            Ok(Value::int(int_prod))
        }
    });

    register_fn(env, "/", |args| {
        check_arity!(args, "/", 2..);
        // Fast path: two integers — use exact i64 division to avoid f64
        // precision loss for values > 2^53, matching the constant folder.
        if args.len() == 2 {
            if let (Some(a), Some(b)) = (args[0].as_int(), args[1].as_int()) {
                if b == 0 {
                    return Err(SemaError::eval("/: division by zero")
                        .with_hint("/: guard with (if (zero? d) ... (/ n d))"));
                }
                if a % b == 0 {
                    return Ok(Value::int(a / b));
                }
                return Ok(Value::float(a as f64 / b as f64));
            }
        }
        let mut result = match args[0].view_ref() {
            ValueViewRef::Int(n) => n as f64,
            ValueViewRef::Float(f) => f,
            _ => return Err(SemaError::type_error("number", args[0].type_name())),
        };
        for arg in &args[1..] {
            let divisor = match arg.view_ref() {
                ValueViewRef::Int(n) => n as f64,
                ValueViewRef::Float(f) => f,
                _ => return Err(SemaError::type_error("number", arg.type_name())),
            };
            if divisor == 0.0 {
                return Err(SemaError::eval("/: division by zero")
                    .with_hint("/: guard with (if (zero? d) ... (/ n d))"));
            }
            result /= divisor;
        }
        if result.fract() == 0.0 && args.iter().all(|a| a.is_int()) {
            Ok(Value::int(result as i64))
        } else {
            Ok(Value::float(result))
        }
    });

    register_fn(env, "mod", mod_impl);
    register_fn(env, "modulo", mod_impl);
}
