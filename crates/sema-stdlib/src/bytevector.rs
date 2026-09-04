use sema_core::{check_arity, ArgsExt, SemaError, Value};

use crate::register_fn;

/// Coerce a `bytes/*` argument to its byte slice.
fn as_bytes<'a>(v: &'a Value, name: &str) -> Result<&'a [u8], SemaError> {
    v.as_bytevector().ok_or_else(|| {
        SemaError::type_error("bytevector", v.type_name()).with_hint(format!(
            "{name}: read one with file/read-bytes or string->utf8"
        ))
    })
}

/// Resolve optional `[start [end]]` arguments (at positions `from..` in
/// `args`) against a buffer of `len` bytes, validating `start <= end <= len`.
fn opt_range(
    args: &[Value],
    from: usize,
    len: usize,
    name: &str,
) -> Result<(usize, usize), SemaError> {
    let start = match args.get(from) {
        Some(v) => v.as_index(name)?,
        None => 0,
    };
    let end = match args.get(from + 1) {
        Some(v) => v.as_index(name)?,
        None => len,
    };
    if start > end || end > len {
        return Err(SemaError::eval(format!(
            "{name}: range {start}..{end} out of bounds for {len} bytes"
        )));
    }
    Ok((start, end))
}

/// First occurrence of `needle` in `hay` (byte index), `None` if absent.
/// An empty needle matches at 0.
fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    match needle.len() {
        0 => Some(0),
        1 => {
            let b = needle[0];
            hay.iter().position(|&x| x == b)
        }
        n => hay.windows(n).position(|w| w == needle),
    }
}

/// Parse ASCII `-?digits(.digit)?` as a base-10 integer scaled by 10
/// (`"-12.3"` → `-123`, `"5"` → `50`) — the 1BRC fixed-point trick:
/// one-decimal temperatures become exact ints with no float math.
fn parse_int10(bytes: &[u8]) -> Result<i64, String> {
    let (neg, rest) = match bytes.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, bytes),
    };
    let overflow = || "value overflows an int".to_string();
    let mut n: i64 = 0;
    let mut i = 0;
    while i < rest.len() && rest[i] != b'.' {
        let b = rest[i];
        if !b.is_ascii_digit() {
            return Err(format!("invalid digit {:?} at byte {i}", b as char));
        }
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add((b - b'0') as i64))
            .ok_or_else(overflow)?;
        i += 1;
    }
    if i == 0 {
        return Err("expected at least one digit".to_string());
    }
    if i < rest.len() {
        // A '.' must be followed by exactly one final digit.
        if i + 2 != rest.len() || !rest[i + 1].is_ascii_digit() {
            return Err("expected exactly one digit after '.'".to_string());
        }
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add((rest[i + 1] - b'0') as i64))
            .ok_or_else(overflow)?;
    } else {
        n = n.checked_mul(10).ok_or_else(overflow)?;
    }
    Ok(if neg { -n } else { n })
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "make-bytevector", |args| {
        check_arity!(args, "make-bytevector", 1..=2);
        let size = args.int_at(0, "make-bytevector")?;
        if size < 0 {
            return Err(SemaError::eval(format!(
                "make-bytevector: size must be non-negative, got {size}"
            )));
        }
        let fill = if args.len() == 2 {
            let f = args.int_at(1, "make-bytevector")?;
            if !(0..=255).contains(&f) {
                return Err(SemaError::eval(format!(
                    "make-bytevector: fill value {f} out of range 0..255"
                )));
            }
            f as u8
        } else {
            0
        };
        Ok(Value::bytevector(vec![fill; size as usize]))
    });

    register_fn(env, "bytevector", |args| {
        let mut bytes = Vec::with_capacity(args.len());
        for (i, arg) in args.iter().enumerate() {
            let n = arg
                .as_int()
                .ok_or_else(|| SemaError::type_error("int", arg.type_name()))?;
            if !(0..=255).contains(&n) {
                return Err(SemaError::eval(format!(
                    "bytevector: byte value {n} at index {i} out of range 0..255"
                )));
            }
            bytes.push(n as u8);
        }
        Ok(Value::bytevector(bytes))
    });

    register_fn(env, "bytevector-length", |args| {
        check_arity!(args, "bytevector-length", 1);
        let bv = args.bytes_at(0, "bytevector-length")?;
        Ok(Value::int(bv.len() as i64))
    });

    register_fn(env, "bytevector-u8-ref", |args| {
        check_arity!(args, "bytevector-u8-ref", 2);
        let bv = args.bytes_at(0, "bytevector-u8-ref")?;
        let idx = args.int_at(1, "bytevector-u8-ref")?;
        if idx < 0 || idx as usize >= bv.len() {
            return Err(SemaError::eval(format!(
                "bytevector-u8-ref: index {idx} out of range for bytevector of length {}",
                bv.len()
            )));
        }
        Ok(Value::int(bv[idx as usize] as i64))
    });

    register_fn(env, "bytevector-u8-set!", |args| {
        check_arity!(args, "bytevector-u8-set!", 3);
        let bv = args.bytes_at(0, "bytevector-u8-set!")?;
        let idx = args.int_at(1, "bytevector-u8-set!")?;
        let byte = args.int_at(2, "bytevector-u8-set!")?;
        if idx < 0 || idx as usize >= bv.len() {
            return Err(SemaError::eval(format!(
                "bytevector-u8-set!: index {idx} out of range for bytevector of length {}",
                bv.len()
            )));
        }
        if !(0..=255).contains(&byte) {
            return Err(SemaError::eval(format!(
                "bytevector-u8-set!: byte value {byte} out of range 0..255"
            )));
        }
        let mut new_bv = bv.to_vec();
        new_bv[idx as usize] = byte as u8;
        Ok(Value::bytevector(new_bv))
    });

    register_fn(env, "bytevector-copy", |args| {
        check_arity!(args, "bytevector-copy", 1..=3);
        let bv = args.bytes_at(0, "bytevector-copy")?;
        let start = if args.len() >= 2 {
            args.int_at(1, "bytevector-copy")? as usize
        } else {
            0
        };
        let end = if args.len() == 3 {
            args.int_at(2, "bytevector-copy")? as usize
        } else {
            bv.len()
        };
        if start > end || end > bv.len() {
            return Err(SemaError::eval(format!(
                "bytevector-copy: range {start}..{end} out of bounds for bytevector of length {}",
                bv.len()
            )));
        }
        Ok(Value::bytevector(bv[start..end].to_vec()))
    });

    register_fn(env, "bytevector-append", |args| {
        let mut result = Vec::new();
        for arg in args {
            let bv = arg
                .as_bytevector()
                .ok_or_else(|| SemaError::type_error("bytevector", arg.type_name()))?;
            result.extend_from_slice(bv);
        }
        Ok(Value::bytevector(result))
    });

    register_fn(env, "bytevector->list", |args| {
        check_arity!(args, "bytevector->list", 1);
        let bv = args.bytes_at(0, "bytevector->list")?;
        let items: Vec<Value> = bv.iter().map(|&b| Value::int(b as i64)).collect();
        Ok(Value::list(items))
    });

    register_fn(env, "list->bytevector", |args| {
        check_arity!(args, "list->bytevector", 1);
        let items = args.list_at(0, "list->bytevector")?;
        let mut bytes = Vec::with_capacity(items.len());
        for item in items {
            let n = item
                .as_int()
                .ok_or_else(|| SemaError::type_error("int", item.type_name()))?;
            if !(0..=255).contains(&n) {
                return Err(SemaError::eval(format!(
                    "list->bytevector: byte value {n} out of range 0..255"
                )));
            }
            bytes.push(n as u8);
        }
        Ok(Value::bytevector(bytes))
    });

    register_fn(env, "utf8->string", |args| {
        check_arity!(args, "utf8->string", 1);
        let bv = args.bytes_at(0, "utf8->string")?;
        let s = String::from_utf8(bv.to_vec())
            .map_err(|e| SemaError::eval(format!("utf8->string: invalid UTF-8: {e}")))?;
        Ok(Value::string(&s))
    });

    register_fn(env, "string->utf8", |args| {
        check_arity!(args, "string->utf8", 1);
        let s = args.str_at(0, "string->utf8")?;
        Ok(Value::bytevector(s.as_bytes().to_vec()))
    });

    // bytes/* — byte-oriented ops on bytevectors for hot loops that avoid
    // UTF-8 work (e.g. the 1BRC pipeline: file/fold-lines-bytes → bytes/find
    // the separator → bytes/parse-int10 the temperature → bytes/->string the
    // key). Optional start/end args index the same bytevector without an
    // intermediate bytes/slice allocation.
    register_fn(env, "bytes/length", |args| {
        check_arity!(args, "bytes/length", 1);
        let bytes = as_bytes(&args[0], "bytes/length")?;
        Ok(Value::int(bytes.len() as i64))
    });

    register_fn(env, "bytes/ref", |args| {
        check_arity!(args, "bytes/ref", 2);
        let bytes = as_bytes(&args[0], "bytes/ref")?;
        let idx = args[1].as_index("bytes/ref")?;
        bytes
            .get(idx)
            .map(|&b| Value::int(b as i64))
            .ok_or_else(|| {
                SemaError::eval(format!(
                    "bytes/ref: index {idx} out of bounds (length {})",
                    bytes.len()
                ))
            })
    });

    register_fn(env, "bytes/find", |args| {
        check_arity!(args, "bytes/find", 2..=3);
        let hay = as_bytes(&args[0], "bytes/find")?;
        let start = match args.get(2) {
            Some(v) => v.as_index("bytes/find")?,
            None => 0,
        };
        if start > hay.len() {
            return Ok(Value::nil());
        }
        let pos = if let Some(b) = args[1].as_int() {
            if !(0..=255).contains(&b) {
                return Err(SemaError::eval(format!(
                    "bytes/find: byte value {b} out of range 0..255"
                )));
            }
            let b = b as u8;
            hay[start..].iter().position(|&x| x == b)
        } else if let Some(needle) = args[1].as_bytevector() {
            find_sub(&hay[start..], needle)
        } else if let Some(s) = args[1].as_str() {
            find_sub(&hay[start..], s.as_bytes())
        } else {
            return Err(
                SemaError::type_error("int, bytevector, or string", args[1].type_name())
                    .with_hint("bytes/find: the needle is a byte (0-255), bytevector, or string"),
            );
        };
        Ok(pos
            .map(|i| Value::int((start + i) as i64))
            .unwrap_or(Value::nil()))
    });

    register_fn(env, "bytes/slice", |args| {
        check_arity!(args, "bytes/slice", 2..=3);
        let bytes = as_bytes(&args[0], "bytes/slice")?;
        let (start, end) = opt_range(args, 1, bytes.len(), "bytes/slice")?;
        Ok(Value::bytevector(bytes[start..end].to_vec()))
    });

    register_fn(env, "bytes/->string", |args| {
        check_arity!(args, "bytes/->string", 1..=3);
        let bytes = as_bytes(&args[0], "bytes/->string")?;
        let (start, end) = opt_range(args, 1, bytes.len(), "bytes/->string")?;
        let s = std::str::from_utf8(&bytes[start..end])
            .map_err(|e| SemaError::eval(format!("bytes/->string: invalid UTF-8: {e}")))?;
        Ok(Value::string(s))
    });

    register_fn(env, "bytes/parse-int10", |args| {
        check_arity!(args, "bytes/parse-int10", 1..=3);
        let bytes = as_bytes(&args[0], "bytes/parse-int10")?;
        let (start, end) = opt_range(args, 1, bytes.len(), "bytes/parse-int10")?;
        parse_int10(&bytes[start..end])
            .map(Value::int)
            .map_err(|e| SemaError::eval(format!("bytes/parse-int10: {e}")))
    });

    // module/function aliases for legacy Scheme names (Decision #24)
    if let Some(v) = env.get(sema_core::intern("make-bytevector")) {
        env.set(sema_core::intern("bytevector/new"), v.clone());
        env.set(sema_core::intern("bytevector/make"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector-length")) {
        env.set(sema_core::intern("bytevector/length"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector-u8-ref")) {
        env.set(sema_core::intern("bytevector/ref"), v.clone());
        env.set(sema_core::intern("bytevector/u8-ref"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector-u8-set!")) {
        env.set(sema_core::intern("bytevector/set!"), v.clone());
        env.set(sema_core::intern("bytevector/u8-set!"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector-copy")) {
        env.set(sema_core::intern("bytevector/copy"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector-append")) {
        env.set(sema_core::intern("bytevector/append"), v);
    }
    if let Some(v) = env.get(sema_core::intern("bytevector->list")) {
        env.set(sema_core::intern("bytevector/to-list"), v);
    }
    if let Some(v) = env.get(sema_core::intern("list->bytevector")) {
        env.set(sema_core::intern("list/to-bytevector"), v.clone());
        env.set(sema_core::intern("bytevector/from-list"), v);
    }
    if let Some(v) = env.get(sema_core::intern("string->utf8")) {
        env.set(sema_core::intern("string/to-utf8"), v.clone());
        // Intuitive name: a Sema string encodes to its UTF-8 bytes.
        env.set(sema_core::intern("string->bytevector"), v);
    }
    if let Some(v) = env.get(sema_core::intern("utf8->string")) {
        env.set(sema_core::intern("utf8/to-string"), v.clone());
        env.set(sema_core::intern("bytevector->string"), v);
    }
}
