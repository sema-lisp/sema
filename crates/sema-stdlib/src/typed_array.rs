use sema_core::{check_arity, SemaError, Value};

use crate::list::{collect_f64_array_call, collect_i64_array_call, register_hof, CollectMode};
use crate::register_fn;

/// Validate a user-supplied array length: a non-negative integer within a sane
/// allocation bound. Without this guard a negative length wraps through
/// `as usize` to a near-`usize::MAX` value and the subsequent `vec![fill; n]`
/// aborts the whole process with a Rust `capacity overflow` panic.
fn array_length(arg: &Value, op: &str) -> Result<usize, SemaError> {
    let n = arg
        .as_int()
        .ok_or_else(|| SemaError::type_error("integer", arg.type_name()))?;
    if n < 0 {
        return Err(SemaError::eval(format!(
            "{op}: length must be non-negative, got {n}"
        )));
    }
    // Cap well below allocation limits so an absurd-but-positive length errors
    // cleanly instead of OOM-killing the process.
    const MAX_LEN: i64 = 1 << 32;
    if n > MAX_LEN {
        return Err(SemaError::eval(format!(
            "{op}: length {n} exceeds maximum {MAX_LEN}"
        )));
    }
    Ok(n as usize)
}

pub fn register(env: &sema_core::Env) {
    // (f64-array/make n) or (f64-array/make n fill) — create f64 array
    register_fn(env, "f64-array/make", |args| {
        check_arity!(args, "f64-array/make", 1..=2);
        let n = array_length(&args[0], "f64-array/make")?;
        let fill = if let Some(v) = args.get(1) {
            v.as_float()
                .or_else(|| v.as_int().map(|i| i as f64))
                .ok_or_else(|| SemaError::type_error("number", v.type_name()))?
        } else {
            0.0
        };
        Ok(Value::f64_array(vec![fill; n]))
    });

    // (i64-array/make n) or (i64-array/make n fill) — create i64 array
    register_fn(env, "i64-array/make", |args| {
        check_arity!(args, "i64-array/make", 1..=2);
        let n = array_length(&args[0], "i64-array/make")?;
        let fill = if let Some(v) = args.get(1) {
            v.as_int()
                .ok_or_else(|| SemaError::type_error("integer", v.type_name()))?
        } else {
            0
        };
        Ok(Value::i64_array(vec![fill; n]))
    });

    // (f64-array vals...) — create from values
    register_fn(env, "f64-array", |args| {
        let mut data = Vec::with_capacity(args.len());
        for arg in args {
            let v = arg
                .as_float()
                .or_else(|| arg.as_int().map(|i| i as f64))
                .ok_or_else(|| SemaError::type_error("number", arg.type_name()))?;
            data.push(v);
        }
        Ok(Value::f64_array(data))
    });

    // (i64-array vals...) — create from values
    register_fn(env, "i64-array", |args| {
        let mut data = Vec::with_capacity(args.len());
        for arg in args {
            let v = arg
                .as_int()
                .ok_or_else(|| SemaError::type_error("integer", arg.type_name()))?;
            data.push(v);
        }
        Ok(Value::i64_array(data))
    });

    // (f64-array/ref arr idx) — get element
    register_fn(env, "f64-array/ref", |args| {
        check_arity!(args, "f64-array/ref", 2);
        let arr = args[0]
            .as_f64_array()
            .ok_or_else(|| SemaError::type_error("f64-array", args[0].type_name()))?;
        let idx = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?
            as usize;
        arr.get(idx).map(|&v| Value::float(v)).ok_or_else(|| {
            SemaError::eval(format!("index {idx} out of bounds (len {})", arr.len()))
        })
    });

    // (i64-array/ref arr idx) — get element
    register_fn(env, "i64-array/ref", |args| {
        check_arity!(args, "i64-array/ref", 2);
        let arr = args[0]
            .as_i64_array()
            .ok_or_else(|| SemaError::type_error("i64-array", args[0].type_name()))?;
        let idx = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?
            as usize;
        arr.get(idx).map(|&v| Value::int(v)).ok_or_else(|| {
            SemaError::eval(format!("index {idx} out of bounds (len {})", arr.len()))
        })
    });

    // (f64-array/set! arr idx val) — set element (returns new array, CoW)
    register_fn(env, "f64-array/set!", |args| {
        check_arity!(args, "f64-array/set!", 3);
        let mut arr = args[0]
            .as_f64_array_rc()
            .ok_or_else(|| SemaError::type_error("f64-array", args[0].type_name()))?;
        let idx = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?
            as usize;
        let val = args[2]
            .as_float()
            .or_else(|| args[2].as_int().map(|i| i as f64))
            .ok_or_else(|| SemaError::type_error("number", args[2].type_name()))?;
        let data = std::rc::Rc::make_mut(&mut arr);
        if idx >= data.len() {
            return Err(SemaError::eval(format!(
                "index {idx} out of bounds (len {})",
                data.len()
            )));
        }
        data[idx] = val;
        Ok(Value::f64_array_from_rc(arr))
    });

    // (i64-array/set! arr idx val) — set element (CoW)
    register_fn(env, "i64-array/set!", |args| {
        check_arity!(args, "i64-array/set!", 3);
        let mut arr = args[0]
            .as_i64_array_rc()
            .ok_or_else(|| SemaError::type_error("i64-array", args[0].type_name()))?;
        let idx = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?
            as usize;
        let val = args[2]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[2].type_name()))?;
        let data = std::rc::Rc::make_mut(&mut arr);
        if idx >= data.len() {
            return Err(SemaError::eval(format!(
                "index {idx} out of bounds (len {})",
                data.len()
            )));
        }
        data[idx] = val;
        Ok(Value::i64_array_from_rc(arr))
    });

    // (f64-array/length arr) — length
    register_fn(env, "f64-array/length", |args| {
        check_arity!(args, "f64-array/length", 1);
        let arr = args[0]
            .as_f64_array()
            .ok_or_else(|| SemaError::type_error("f64-array", args[0].type_name()))?;
        Ok(Value::int(arr.len() as i64))
    });

    // (i64-array/length arr) — length
    register_fn(env, "i64-array/length", |args| {
        check_arity!(args, "i64-array/length", 1);
        let arr = args[0]
            .as_i64_array()
            .ok_or_else(|| SemaError::type_error("i64-array", args[0].type_name()))?;
        Ok(Value::int(arr.len() as i64))
    });

    // (f64-array/sum arr) — fast sum without boxing overhead
    register_fn(env, "f64-array/sum", |args| {
        check_arity!(args, "f64-array/sum", 1);
        let arr = args[0]
            .as_f64_array()
            .ok_or_else(|| SemaError::type_error("f64-array", args[0].type_name()))?;
        Ok(Value::float(arr.iter().sum::<f64>()))
    });

    // (i64-array/sum arr) — fast sum
    register_fn(env, "i64-array/sum", |args| {
        check_arity!(args, "i64-array/sum", 1);
        let arr = args[0]
            .as_i64_array()
            .ok_or_else(|| SemaError::type_error("i64-array", args[0].type_name()))?;
        Ok(Value::int(arr.iter().sum::<i64>()))
    });

    // (f64-array/dot a b) — dot product, fast inner loop in Rust
    register_fn(env, "f64-array/dot", |args| {
        check_arity!(args, "f64-array/dot", 2);
        let a = args[0]
            .as_f64_array()
            .ok_or_else(|| SemaError::type_error("f64-array", args[0].type_name()))?;
        let b = args[1]
            .as_f64_array()
            .ok_or_else(|| SemaError::type_error("f64-array", args[1].type_name()))?;
        if a.len() != b.len() {
            return Err(SemaError::eval(format!(
                "f64-array/dot: length mismatch ({} vs {})",
                a.len(),
                b.len()
            )));
        }
        let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        Ok(Value::float(dot))
    });

    // (f64-array/map f arr) — apply f to each element, return new array
    register_hof(
        env,
        "f64-array/map",
        |args| {
            check_arity!(args, "f64-array/map", 2);
            let f = &args[0];
            let arr = args[1]
                .as_f64_array()
                .ok_or_else(|| SemaError::type_error("f64-array", args[1].type_name()))?;
            let mut result = Vec::with_capacity(arr.len());
            for &v in arr.iter() {
                let out = crate::list::call_function(f, &[Value::float(v)])?;
                let fval = out
                    .as_float()
                    .or_else(|| out.as_int().map(|i| i as f64))
                    .ok_or_else(|| {
                        SemaError::type_error(
                            "number (f64-array/map callback must return number)",
                            out.type_name(),
                        )
                    })?;
                result.push(fval);
            }
            Ok(Value::f64_array(result))
        },
        |args| {
            check_arity!(args, "f64-array/map", 2);
            args[1]
                .as_f64_array()
                .ok_or_else(|| SemaError::type_error("f64-array", args[1].type_name()))?;
            Ok(collect_f64_array_call(
                &args[0],
                args[1].clone(),
                CollectMode::F64Array,
                "f64-array/map",
            ))
        },
    );

    // (i64-array/map f arr) — apply f to each element, return new array
    register_hof(
        env,
        "i64-array/map",
        |args| {
            check_arity!(args, "i64-array/map", 2);
            let f = &args[0];
            let arr = args[1]
                .as_i64_array()
                .ok_or_else(|| SemaError::type_error("i64-array", args[1].type_name()))?;
            let mut result = Vec::with_capacity(arr.len());
            for &v in arr.iter() {
                let out = crate::list::call_function(f, &[Value::int(v)])?;
                let ival = out.as_int().ok_or_else(|| {
                    SemaError::type_error(
                        "integer (i64-array/map callback must return integer)",
                        out.type_name(),
                    )
                })?;
                result.push(ival);
            }
            Ok(Value::i64_array(result))
        },
        |args| {
            check_arity!(args, "i64-array/map", 2);
            args[1]
                .as_i64_array()
                .ok_or_else(|| SemaError::type_error("i64-array", args[1].type_name()))?;
            Ok(collect_i64_array_call(
                &args[0],
                args[1].clone(),
                CollectMode::I64Array,
                "i64-array/map",
            ))
        },
    );

    // (f64-array/fold f init arr) — fold over array
    register_fn(env, "f64-array/fold", |args| {
        check_arity!(args, "f64-array/fold", 3);
        let f = &args[0];
        let mut acc = args[1].clone();
        let arr = args[2]
            .as_f64_array()
            .ok_or_else(|| SemaError::type_error("f64-array", args[2].type_name()))?;
        for &v in arr.iter() {
            acc = crate::list::call_function(f, &[acc, Value::float(v)])?;
        }
        Ok(acc)
    });

    // (i64-array/fold f init arr) — fold over array
    register_fn(env, "i64-array/fold", |args| {
        check_arity!(args, "i64-array/fold", 3);
        let f = &args[0];
        let mut acc = args[1].clone();
        let arr = args[2]
            .as_i64_array()
            .ok_or_else(|| SemaError::type_error("i64-array", args[2].type_name()))?;
        for &v in arr.iter() {
            acc = crate::list::call_function(f, &[acc, Value::int(v)])?;
        }
        Ok(acc)
    });

    // (f64-array/from-list lst) — convert list of numbers to f64 array
    register_fn(env, "f64-array/from-list", |args| {
        check_arity!(args, "f64-array/from-list", 1);
        let lst = args[0]
            .as_list()
            .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;
        let mut data = Vec::with_capacity(lst.len());
        for v in lst.iter() {
            let f = v
                .as_float()
                .or_else(|| v.as_int().map(|i| i as f64))
                .ok_or_else(|| SemaError::type_error("number", v.type_name()))?;
            data.push(f);
        }
        Ok(Value::f64_array(data))
    });

    // (i64-array/from-list lst) — convert list of ints to i64 array
    register_fn(env, "i64-array/from-list", |args| {
        check_arity!(args, "i64-array/from-list", 1);
        let lst = args[0]
            .as_list()
            .ok_or_else(|| SemaError::type_error("list", args[0].type_name()))?;
        let mut data = Vec::with_capacity(lst.len());
        for v in lst.iter() {
            let i = v
                .as_int()
                .ok_or_else(|| SemaError::type_error("integer", v.type_name()))?;
            data.push(i);
        }
        Ok(Value::i64_array(data))
    });

    // Type predicates
    register_fn(env, "f64-array?", |args| {
        check_arity!(args, "f64-array?", 1);
        Ok(Value::bool(args[0].as_f64_array().is_some()))
    });

    register_fn(env, "i64-array?", |args| {
        check_arity!(args, "i64-array?", 1);
        Ok(Value::bool(args[0].as_i64_array().is_some()))
    });

    // (f64-array/range start end) or (f64-array/range start end step) — numeric range as array
    register_fn(env, "f64-array/range", |args| {
        check_arity!(args, "f64-array/range", 2..=3);
        let start = args[0]
            .as_float()
            .or_else(|| args[0].as_int().map(|i| i as f64))
            .ok_or_else(|| SemaError::type_error("number", args[0].type_name()))?;
        let end = args[1]
            .as_float()
            .or_else(|| args[1].as_int().map(|i| i as f64))
            .ok_or_else(|| SemaError::type_error("number", args[1].type_name()))?;
        let step = if let Some(v) = args.get(2) {
            v.as_float()
                .or_else(|| v.as_int().map(|i| i as f64))
                .ok_or_else(|| SemaError::type_error("number", v.type_name()))?
        } else {
            1.0
        };
        if step == 0.0 {
            return Err(SemaError::eval("f64-array/range: step cannot be zero"));
        }
        let n = ((end - start) / step).ceil().max(0.0) as usize;
        let mut data = Vec::with_capacity(n);
        let mut v = start;
        if step > 0.0 {
            while v < end {
                data.push(v);
                v += step;
            }
        } else {
            while v > end {
                data.push(v);
                v += step;
            }
        }
        Ok(Value::f64_array(data))
    });

    // (i64-array/range start end) — integer range as array
    register_fn(env, "i64-array/range", |args| {
        check_arity!(args, "i64-array/range", 2..=3);
        let start = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
        let end = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?;
        let step = if let Some(v) = args.get(2) {
            v.as_int()
                .ok_or_else(|| SemaError::type_error("integer", v.type_name()))?
        } else {
            1
        };
        if step == 0 {
            return Err(SemaError::eval("i64-array/range: step cannot be zero"));
        }
        let mut data = Vec::new();
        let mut v = start;
        if step > 0 {
            while v < end {
                data.push(v);
                v += step;
            }
        } else {
            while v > end {
                data.push(v);
                v += step;
            }
        }
        Ok(Value::i64_array(data))
    });
}
