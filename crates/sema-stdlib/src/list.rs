use sema_core::{check_arity, SemaError, Value, ValueViewRef};

use crate::register_fn;

/// Sort category of a value for the comparator-free `sort`. Ints and floats
/// share the `Number` family (they must compare by numeric value, not by tag);
/// every other type is only comparable to its own kind. `sort` refuses to order
/// values whose categories differ, because `Value`'s cross-type `Ord` falls back
/// to an internal tag order that is arbitrary and never what the caller meant.
#[derive(PartialEq, Eq)]
enum SortCategory {
    Number,
    Other(&'static str),
}

fn sort_category(v: &Value) -> SortCategory {
    if v.is_int() || v.is_float() {
        SortCategory::Number
    } else {
        SortCategory::Other(v.type_name())
    }
}

fn repeat_impl(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "list/repeat", 2);
    let n = args[0].as_index("list/repeat")?;
    let val = args[1].clone();
    Ok(Value::list(vec![val; n]))
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "list", |args| Ok(Value::list(args.to_vec())));

    register_fn(env, "vector", |args| Ok(Value::vector(args.to_vec())));

    register_fn(env, "cons", |args| {
        check_arity!(args, "cons", 2);
        if args[1].is_nil() {
            Ok(Value::list(vec![args[0].clone()]))
        } else if let Some(list) = args[1].as_list() {
            let mut new = vec![args[0].clone()];
            new.extend(list.iter().cloned());
            Ok(Value::list(new))
        } else {
            Ok(Value::list(vec![args[0].clone(), args[1].clone()]))
        }
    });

    register_fn(env, "car", first);
    register_fn(env, "first", first);

    register_fn(env, "cdr", rest);
    register_fn(env, "rest", rest);

    register_fn(env, "length", |args| {
        check_arity!(args, "length", 1);
        if let Some(l) = args[0].as_list() {
            Ok(Value::int(l.len() as i64))
        } else if let Some(v) = args[0].as_vector() {
            Ok(Value::int(v.len() as i64))
        } else if let Some(s) = args[0].as_str() {
            Ok(Value::int(s.chars().count() as i64))
        } else if let Some(m) = args[0].as_map_rc() {
            Ok(Value::int(m.len() as i64))
        } else if let Some(m) = args[0].as_hashmap_rc() {
            Ok(Value::int(m.len() as i64))
        } else if let Some(bv) = args[0].as_bytevector() {
            Ok(Value::int(bv.len() as i64))
        } else if let Some(arr) = args[0].as_f64_array() {
            Ok(Value::int(arr.len() as i64))
        } else if let Some(arr) = args[0].as_i64_array() {
            Ok(Value::int(arr.len() as i64))
        } else {
            Err(SemaError::type_error(
                "list, vector, string, map, bytevector, or typed array",
                args[0].type_name(),
            )
            .with_hint("length: expected a sequence or collection"))
        }
    });

    register_fn(env, "append", |args| {
        let mut result = Vec::new();
        for arg in args {
            if let Some(l) = arg.as_list() {
                result.extend(l.iter().cloned());
            } else if let Some(v) = arg.as_vector() {
                result.extend(v.iter().cloned());
            } else {
                return Err(SemaError::type_error("list or vector", arg.type_name())
                    .with_hint("append: every argument must be a list or vector"));
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "reverse", |args| {
        check_arity!(args, "reverse", 1);
        if let Some(l) = args[0].as_list() {
            let mut v = l.to_vec();
            v.reverse();
            Ok(Value::list(v))
        } else if let Some(v) = args[0].as_vector() {
            let mut items = v.to_vec();
            items.reverse();
            Ok(Value::vector(items))
        } else {
            Err(SemaError::type_error("list or vector", args[0].type_name())
                .with_hint("reverse: argument 1 must be a list or vector"))
        }
    });

    register_fn(env, "nth", |args| {
        check_arity!(args, "nth", 2);
        let idx_i = args[1].as_int().ok_or_else(|| {
            // A collection in the index slot almost always means swapped args.
            let swapped = args[1].as_list().is_some() || args[1].as_vector().is_some();
            let hint = if swapped {
                "nth: argument order is (nth collection index) — looks like the arguments are swapped"
            } else {
                "nth: argument order is (nth collection index); the index must be an integer"
            };
            SemaError::type_error("int", args[1].type_name()).with_hint(hint)
        })?;
        if idx_i < 0 {
            return Err(
                SemaError::eval(format!("nth: index must be non-negative, got {idx_i}"))
                    .with_hint("indices are 0-based; use (last xs) for the last element"),
            );
        }
        let idx = idx_i as usize;
        if let Some(l) = args[0].as_list() {
            l.get(idx).cloned().ok_or_else(|| {
                SemaError::eval(format!("index {idx} out of bounds (length {})", l.len()))
            })
        } else if let Some(v) = args[0].as_vector() {
            v.get(idx).cloned().ok_or_else(|| {
                SemaError::eval(format!("index {idx} out of bounds (length {})", v.len()))
            })
        } else {
            Err(SemaError::type_error("list or vector", args[0].type_name())
                .with_hint("nth: argument 1 must be a list or vector"))
        }
    });

    register_fn(env, "map", |args| {
        check_arity!(args, "map", 2..);
        if args.len() == 2 {
            let items = get_sequence(&args[1], "map")?;
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                result.push(call_function(&args[0], &[item.clone()])?);
            }
            Ok(Value::list(result))
        } else {
            // Multi-list map: iterate in lockstep (shortest wins)
            let lists: Vec<&[Value]> = args[1..]
                .iter()
                .map(|a| get_sequence(a, "map"))
                .collect::<Result<_, _>>()?;
            let min_len = lists.iter().map(|l| l.len()).min().unwrap_or(0);
            let mut result = Vec::with_capacity(min_len);
            for i in 0..min_len {
                let call_args: Vec<Value> = lists.iter().map(|l| l[i].clone()).collect();
                result.push(call_function(&args[0], &call_args)?);
            }
            Ok(Value::list(result))
        }
    });

    register_fn(env, "filter", |args| {
        check_arity!(args, "filter", 2);
        let items = get_sequence(&args[1], "filter")?;
        let mut result = Vec::new();
        for item in items {
            let owned = item.clone();
            let keep = call_function(&args[0], std::slice::from_ref(&owned))?;
            if keep.is_truthy() {
                result.push(owned);
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "foldl", |args| {
        check_arity!(args, "foldl", 3);
        let items = get_sequence(&args[2], "foldl")?;
        let mut acc = args[1].clone();
        for item in items {
            acc = call_function(&args[0], &[acc, item.clone()])?;
        }
        Ok(acc)
    });

    register_fn(env, "for-each", |args| {
        check_arity!(args, "for-each", 2);
        let items = get_sequence(&args[1], "for-each")?;
        for item in items {
            call_function(&args[0], &[item.clone()])?;
        }
        Ok(Value::nil())
    });

    register_fn(env, "range", |args| {
        check_arity!(args, "range", 1..=3);
        let (start, end, step) = match args.len() {
            1 => (
                0i64,
                args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?,
                1i64,
            ),
            2 => {
                let s = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                let e = args[1]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
                (s, e, 1)
            }
            _ => {
                let s = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                let e = args[1]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
                let st = args[2]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[2].type_name()))?;
                (s, e, st)
            }
        };
        if step == 0 {
            return Err(SemaError::eval("range: step cannot be 0"));
        }
        let mut result = Vec::new();
        let mut i = start;
        if step > 0 {
            while i < end {
                result.push(Value::int(i));
                i += step;
            }
        } else {
            while i > end {
                result.push(Value::int(i));
                i += step;
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "apply", |args| {
        check_arity!(args, "apply", 2..);
        let func = &args[0];
        // Last arg must be a list, preceding args are prepended
        let last = &args[args.len() - 1];
        let last_items = get_sequence(last, "apply")?;
        let mut all_args: Vec<Value> = args[1..args.len() - 1].to_vec();
        all_args.extend(last_items.iter().cloned());
        call_function(func, &all_args)
    });

    register_fn(env, "take", |args| {
        check_arity!(args, "take", 2);
        let n = args[0].as_index("take")?;
        let items = get_sequence(&args[1], "take")?;
        let end = n.min(items.len());
        Ok(Value::list(items[..end].to_vec()))
    });

    register_fn(env, "drop", |args| {
        check_arity!(args, "drop", 2);
        let n = args[0].as_index("drop")?;
        let items = get_sequence(&args[1], "drop")?;
        let start = n.min(items.len());
        Ok(Value::list(items[start..].to_vec()))
    });

    register_fn(env, "last", |args| {
        check_arity!(args, "last", 1);
        let items = get_sequence(&args[0], "last")?;
        Ok(items.last().cloned().unwrap_or(Value::nil()))
    });

    register_fn(env, "zip", |args| {
        check_arity!(args, "zip", 2..);
        let lists: Vec<&[Value]> = args
            .iter()
            .map(|a| get_sequence(a, "zip"))
            .collect::<Result<_, _>>()?;
        let min_len = lists.iter().map(|l| l.len()).min().unwrap_or(0);
        let mut result = Vec::with_capacity(min_len);
        for i in 0..min_len {
            let tuple: Vec<Value> = lists.iter().map(|l| l[i].clone()).collect();
            result.push(Value::list(tuple));
        }
        Ok(Value::list(result))
    });

    register_fn(env, "flatten", |args| {
        check_arity!(args, "flatten", 1);
        let items = get_sequence(&args[0], "flatten")?;
        let mut result = Vec::new();
        for item in items {
            if let Some(l) = item.as_list() {
                result.extend(l.iter().cloned());
            } else if let Some(v) = item.as_vector() {
                result.extend(v.iter().cloned());
            } else {
                result.push(item.clone());
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "member", |args| {
        check_arity!(args, "member", 2);
        let items = get_sequence(&args[1], "member")?;
        for (i, item) in items.iter().enumerate() {
            if item == &args[0] {
                return Ok(Value::list(items[i..].to_vec()));
            }
        }
        Ok(Value::bool(false))
    });

    register_fn(env, "any", |args| {
        check_arity!(args, "any", 2);
        let items = get_sequence(&args[1], "any")?;
        for item in items {
            if call_function(&args[0], &[item.clone()])?.is_truthy() {
                return Ok(Value::bool(true));
            }
        }
        Ok(Value::bool(false))
    });

    register_fn(env, "every", |args| {
        check_arity!(args, "every", 2);
        let items = get_sequence(&args[1], "every")?;
        for item in items {
            if !call_function(&args[0], &[item.clone()])?.is_truthy() {
                return Ok(Value::bool(false));
            }
        }
        Ok(Value::bool(true))
    });
    // Note: canonical predicate-? aliases (`any?`, `every?`) are registered
    // at the end of this fn (see below).

    register_fn(env, "reduce", |args| {
        check_arity!(args, "reduce", 2);
        let items = get_sequence(&args[1], "reduce")?;
        if items.is_empty() {
            return Err(SemaError::eval("reduce: empty list"));
        }
        let mut acc = items[0].clone();
        for item in &items[1..] {
            acc = call_function(&args[0], &[acc, item.clone()])?;
        }
        Ok(acc)
    });

    register_fn(env, "partition", |args| {
        check_arity!(args, "partition", 2);
        let items = get_sequence(&args[1], "partition")?;
        let mut matching = Vec::new();
        let mut non_matching = Vec::new();
        for item in items {
            if call_function(&args[0], &[item.clone()])?.is_truthy() {
                matching.push(item.clone());
            } else {
                non_matching.push(item.clone());
            }
        }
        Ok(Value::list(vec![
            Value::list(matching),
            Value::list(non_matching),
        ]))
    });

    register_fn(env, "foldr", |args| {
        check_arity!(args, "foldr", 3);
        let items = get_sequence(&args[2], "foldr")?;
        let mut acc = args[1].clone();
        for item in items.iter().rev() {
            acc = call_function(&args[0], &[item.clone(), acc])?;
        }
        Ok(acc)
    });

    register_fn(env, "sort", |args| {
        check_arity!(args, "sort", 1..=2);
        let mut items = get_sequence(&args[0], "sort")?.to_vec();
        if args.len() == 1 {
            // Reject heterogeneous input up front: comparing across unrelated
            // types would silently fall back to `Value`'s arbitrary tag order.
            // Pass an explicit comparator (`sort-by` / 2-arg `sort`) to order
            // mixed types deliberately.
            if let Some(first) = items.first() {
                let cat = sort_category(first);
                if let Some(bad) = items.iter().find(|v| sort_category(v) != cat) {
                    return Err(SemaError::type_error(first.type_name(), bad.type_name())
                        .with_hint(
                            "sort orders one type at a time; use `sort-by` or `(sort xs cmp)` \
                             with a comparator to order mixed types",
                        ));
                }
            }
            // All-number lists must compare by numeric value: `Value`'s `Ord`
            // orders every int before every float regardless of magnitude, so
            // `(sort (list 3 1.5))` would otherwise misorder. Floats use a total
            // order (NaN last) to keep the sort well-defined.
            if matches!(items.first().map(sort_category), Some(SortCategory::Number)) {
                items.sort_by(|a, b| {
                    let x = a.as_float().unwrap();
                    let y = b.as_float().unwrap();
                    x.total_cmp(&y)
                });
            } else {
                items.sort();
            }
        } else {
            // Sort with comparator
            let mut err = None;
            items.sort_by(|a, b| {
                if err.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                match call_function(&args[1], &[a.clone(), b.clone()]) {
                    Ok(ref v) if v.is_int() => {
                        let n = v.as_int().unwrap();
                        if n < 0 {
                            std::cmp::Ordering::Less
                        } else if n > 0 {
                            std::cmp::Ordering::Greater
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    }
                    Ok(ref v) if v.as_bool() == Some(true) => std::cmp::Ordering::Less,
                    Ok(ref v) if v.as_bool() == Some(false) => std::cmp::Ordering::Greater,
                    Ok(_) => std::cmp::Ordering::Equal,
                    Err(e) => {
                        err = Some(e);
                        std::cmp::Ordering::Equal
                    }
                }
            });
            if let Some(e) = err {
                return Err(e);
            }
        }
        Ok(Value::list(items))
    });

    register_fn(env, "list/index-of", |args| {
        check_arity!(args, "list/index-of", 2);
        let items = get_sequence(&args[0], "list/index-of")?;
        for (i, item) in items.iter().enumerate() {
            if item == &args[1] {
                return Ok(Value::int(i as i64));
            }
        }
        Ok(Value::nil())
    });

    // Boolean membership — unlike `member` (which returns the Scheme tail-or-#f), this
    // reads as a predicate and allocates nothing.
    register_fn(env, "list/contains?", |args| {
        check_arity!(args, "list/contains?", 2);
        let items = get_sequence(&args[0], "list/contains?")?;
        Ok(Value::bool(items.iter().any(|item| item == &args[1])))
    });

    // Safe indexed access: returns `default` instead of erroring when out of bounds.
    register_fn(env, "list/nth-or", |args| {
        check_arity!(args, "list/nth-or", 3);
        let items = get_sequence(&args[0], "list/nth-or")?;
        let idx = args[1].as_index("list/nth-or")?;
        Ok(items.get(idx).cloned().unwrap_or_else(|| args[2].clone()))
    });

    // The last `n` elements (inverse of `take`). Clamps to the sequence length.
    register_fn(env, "list/take-last", |args| {
        check_arity!(args, "list/take-last", 2);
        let n = args[0].as_index("list/take-last")?;
        let items = get_sequence(&args[1], "list/take-last")?;
        let start = items.len().saturating_sub(n);
        Ok(Value::list(items[start..].to_vec()))
    });

    // All but the last `n` elements (drop from the tail). Clamps to empty.
    register_fn(env, "list/drop-last", |args| {
        check_arity!(args, "list/drop-last", 2);
        let n = args[0].as_index("list/drop-last")?;
        let items = get_sequence(&args[1], "list/drop-last")?;
        let end = items.len().saturating_sub(n);
        Ok(Value::list(items[..end].to_vec()))
    });

    register_fn(env, "list/unique", |args| {
        check_arity!(args, "list/unique", 1);
        let items = get_sequence(&args[0], "list/unique")?;
        let mut seen: std::collections::BTreeSet<Value> = std::collections::BTreeSet::new();
        let mut result = Vec::new();
        for item in items {
            if seen.insert(item.clone()) {
                result.push(item.clone());
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "list/group-by", |args| {
        check_arity!(args, "list/group-by", 2);
        let items = get_sequence(&args[1], "list/group-by")?;
        let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
        for item in items {
            let key = call_function(&args[0], &[item.clone()])?;
            if let Some(group) = groups.iter_mut().find(|(k, _)| k == &key) {
                group.1.push(item.clone());
            } else {
                groups.push((key, vec![item.clone()]));
            }
        }
        let mut map = std::collections::BTreeMap::new();
        for (key, vals) in groups {
            map.insert(key, Value::list(vals));
        }
        Ok(Value::map(map))
    });

    register_fn(env, "list/interleave", |args| {
        check_arity!(args, "list/interleave", 2..);
        let lists: Vec<&[Value]> = args
            .iter()
            .map(|a| get_sequence(a, "list/interleave"))
            .collect::<Result<_, _>>()?;
        let min_len = lists.iter().map(|l| l.len()).min().unwrap_or(0);
        let mut result = Vec::with_capacity(min_len * lists.len());
        for i in 0..min_len {
            for list in &lists {
                result.push(list[i].clone());
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "sort-by", |args| {
        check_arity!(args, "sort-by", 2);
        let items = get_sequence(&args[1], "sort-by")?;
        // Extract keys for each element
        let mut keyed: Vec<(Value, Value)> = Vec::with_capacity(items.len());
        for item in items {
            let key = call_function(&args[0], &[item.clone()])?;
            keyed.push((key, item.clone()));
        }
        keyed.sort_by(|(ka, _), (kb, _)| ka.cmp(kb));
        let result: Vec<Value> = keyed.into_iter().map(|(_, v)| v).collect();
        Ok(Value::list(result))
    });

    register_fn(env, "flatten-deep", |args| {
        check_arity!(args, "flatten-deep", 1);
        let mut out = Vec::new();
        flatten_recursive(&args[0], &mut out);
        Ok(Value::list(out))
    });

    register_fn(env, "interpose", |args| {
        check_arity!(args, "interpose", 2);
        let items = get_sequence(&args[1], "interpose")?;
        if items.is_empty() {
            return Ok(Value::list(vec![]));
        }
        let mut result = Vec::with_capacity(items.len() * 2 - 1);
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                result.push(args[0].clone());
            }
            result.push(item.clone());
        }
        Ok(Value::list(result))
    });

    register_fn(env, "frequencies", |args| {
        check_arity!(args, "frequencies", 1);
        let items = get_sequence(&args[0], "frequencies")?;
        let mut counts: std::collections::BTreeMap<Value, i64> = std::collections::BTreeMap::new();
        for item in items {
            *counts.entry(item.clone()).or_insert(0) += 1;
        }
        let map: std::collections::BTreeMap<Value, Value> = counts
            .into_iter()
            .map(|(k, v)| (k, Value::int(v)))
            .collect();
        Ok(Value::map(map))
    });

    register_fn(env, "list->vector", |args| {
        check_arity!(args, "list->vector", 1);
        if let Some(l) = args[0].as_list() {
            Ok(Value::vector(l.to_vec()))
        } else {
            Err(SemaError::type_error("list", args[0].type_name())
                .with_hint("list->vector: argument 1 must be a list"))
        }
    });

    register_fn(env, "vector->list", |args| {
        check_arity!(args, "vector->list", 1);
        if let Some(v) = args[0].as_vector() {
            Ok(Value::list(v.to_vec()))
        } else {
            Err(SemaError::type_error("vector", args[0].type_name())
                .with_hint("vector->list: argument 1 must be a vector"))
        }
    });

    register_fn(env, "list/chunk", |args| {
        check_arity!(args, "list/chunk", 2);
        let n = args[0].as_index("list/chunk")?;
        if n == 0 {
            return Err(SemaError::eval("list/chunk: chunk size must be positive"));
        }
        let items = get_sequence(&args[1], "list/chunk")?;
        let mut result = Vec::new();
        for chunk in items.chunks(n) {
            result.push(Value::list(chunk.to_vec()));
        }
        Ok(Value::list(result))
    });

    register_fn(env, "take-while", |args| {
        check_arity!(args, "take-while", 2);
        let items = get_sequence(&args[1], "take-while")?;
        let mut result = Vec::new();
        for item in items {
            if call_function(&args[0], &[item.clone()])?.is_truthy() {
                result.push(item.clone());
            } else {
                break;
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "drop-while", |args| {
        check_arity!(args, "drop-while", 2);
        let items = get_sequence(&args[1], "drop-while")?;
        let mut dropping = true;
        let mut result = Vec::new();
        for item in items {
            if dropping && call_function(&args[0], &[item.clone()])?.is_truthy() {
                continue;
            }
            dropping = false;
            result.push(item.clone());
        }
        Ok(Value::list(result))
    });

    register_fn(env, "list/dedupe", |args| {
        check_arity!(args, "list/dedupe", 1);
        let items = get_sequence(&args[0], "list/dedupe")?;
        let mut result = Vec::new();
        for item in items {
            if result.last() != Some(item) {
                result.push(item.clone());
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "flat-map", |args| {
        check_arity!(args, "flat-map", 2);
        let items = get_sequence(&args[1], "flat-map")?;
        let mut result = Vec::new();
        for item in items {
            let mapped = call_function(&args[0], &[item.clone()])?;
            if let Some(l) = mapped.as_list() {
                result.extend(l.iter().cloned());
            } else if let Some(v) = mapped.as_vector() {
                result.extend(v.iter().cloned());
            } else {
                result.push(mapped);
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "list/shuffle", |args| {
        check_arity!(args, "list/shuffle", 1);
        let mut items = get_sequence(&args[0], "list/shuffle")?.to_vec();
        use rand::seq::SliceRandom;
        items.shuffle(&mut rand::rng());
        Ok(Value::list(items))
    });

    register_fn(env, "list/split-at", |args| {
        check_arity!(args, "list/split-at", 2);
        let items = get_sequence(&args[0], "list/split-at")?;
        let n = args[1].as_index("list/split-at")?;
        let n = n.min(items.len());
        let left = items[..n].to_vec();
        let right = items[n..].to_vec();
        Ok(Value::list(vec![Value::list(left), Value::list(right)]))
    });

    register_fn(env, "list/take-while", |args| {
        check_arity!(args, "list/take-while", 2);
        let items = get_sequence(&args[1], "list/take-while")?;
        let mut result = Vec::new();
        for item in items {
            let keep = call_function(&args[0], &[item.clone()])?;
            if keep.is_truthy() {
                result.push(item.clone());
            } else {
                break;
            }
        }
        Ok(Value::list(result))
    });

    register_fn(env, "list/drop-while", |args| {
        check_arity!(args, "list/drop-while", 2);
        let items = get_sequence(&args[1], "list/drop-while")?;
        let mut dropping = true;
        let mut result = Vec::new();
        for item in items {
            if dropping {
                let drop = call_function(&args[0], &[item.clone()])?;
                if drop.is_truthy() {
                    continue;
                }
                dropping = false;
            }
            result.push(item.clone());
        }
        Ok(Value::list(result))
    });

    register_fn(env, "list/sum", |args| {
        check_arity!(args, "list/sum", 1);
        let items = get_sequence(&args[0], "list/sum")?;
        let mut int_sum: i64 = 0;
        let mut has_float = false;
        let mut float_sum: f64 = 0.0;
        for item in items {
            if let Some(n) = item.as_int() {
                int_sum += n;
                float_sum += n as f64;
            } else if let Some(f) = item.as_float() {
                has_float = true;
                float_sum += f;
            } else {
                return Err(SemaError::type_error("number", item.type_name()));
            }
        }
        if has_float {
            Ok(Value::float(float_sum))
        } else {
            Ok(Value::int(int_sum))
        }
    });

    register_fn(env, "list/min", |args| {
        check_arity!(args, "list/min", 1);
        let items = get_sequence(&args[0], "list/min")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/min: empty list"));
        }
        let mut result = items[0].clone();
        for item in &items[1..] {
            if num_lt(item, &result)? {
                result = item.clone();
            }
        }
        Ok(result)
    });

    register_fn(env, "list/max", |args| {
        check_arity!(args, "list/max", 1);
        let items = get_sequence(&args[0], "list/max")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/max: empty list"));
        }
        let mut result = items[0].clone();
        for item in &items[1..] {
            if num_lt(&result, item)? {
                result = item.clone();
            }
        }
        Ok(result)
    });

    register_fn(env, "list/pick", |args| {
        check_arity!(args, "list/pick", 1);
        let items = get_sequence(&args[0], "list/pick")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/pick: empty list"));
        }
        use rand::seq::IndexedRandom;
        let chosen = items.choose(&mut rand::rng()).unwrap();
        Ok(chosen.clone())
    });

    register_fn(env, "list/repeat", repeat_impl);
    register_fn(env, "make-list", repeat_impl);

    register_fn(env, "iota", |args| {
        check_arity!(args, "iota", 1..=3);
        let (count, start, step) = match args.len() {
            1 => {
                let c = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                (c, 0i64, 1i64)
            }
            2 => {
                let c = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                let s = args[1]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
                (c, s, 1)
            }
            _ => {
                let c = args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
                let s = args[1]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
                let st = args[2]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[2].type_name()))?;
                (c, s, st)
            }
        };
        let mut result = Vec::with_capacity(count.max(0) as usize);
        let mut val = start;
        for _ in 0..count {
            result.push(Value::int(val));
            val += step;
        }
        Ok(Value::list(result))
    });

    // list/reject — inverse of filter
    register_fn(env, "list/reject", |args| {
        check_arity!(args, "list/reject", 2);
        let items = get_sequence(&args[1], "list/reject")?;
        let mut result = Vec::new();
        for item in items {
            let reject = call_function(&args[0], &[item.clone()])?;
            if !reject.is_truthy() {
                result.push(item.clone());
            }
        }
        Ok(Value::list(result))
    });

    // list/pluck — extract a field from list of maps
    register_fn(env, "list/pluck", |args| {
        check_arity!(args, "list/pluck", 2);
        let key = &args[0];
        let items = get_sequence(&args[1], "list/pluck")?;
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            let val = match item.view_ref() {
                ValueViewRef::Map(m) => m.get(key).cloned().unwrap_or(Value::nil()),
                ValueViewRef::HashMap(m) => m.get(key).cloned().unwrap_or(Value::nil()),
                _ => Value::nil(),
            };
            result.push(val);
        }
        Ok(Value::list(result))
    });

    // list/avg — average of numeric list
    register_fn(env, "list/avg", |args| {
        check_arity!(args, "list/avg", 1);
        let items = get_sequence(&args[0], "list/avg")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/avg: empty list"));
        }
        let mut sum: f64 = 0.0;
        for item in items {
            if let Some(n) = item.as_int() {
                sum += n as f64;
            } else if let Some(f) = item.as_float() {
                sum += f;
            } else {
                return Err(SemaError::type_error("number", item.type_name()));
            }
        }
        Ok(Value::float(sum / items.len() as f64))
    });

    // list/median — statistical median
    register_fn(env, "list/median", |args| {
        check_arity!(args, "list/median", 1);
        let items = get_sequence(&args[0], "list/median")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/median: empty list"));
        }
        let mut nums: Vec<f64> = Vec::with_capacity(items.len());
        for item in items {
            if let Some(n) = item.as_int() {
                nums.push(n as f64);
            } else if let Some(f) = item.as_float() {
                nums.push(f);
            } else {
                return Err(SemaError::type_error("number", item.type_name()));
            }
        }
        nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = nums.len() / 2;
        if nums.len().is_multiple_of(2) {
            Ok(Value::float((nums[mid - 1] + nums[mid]) / 2.0))
        } else {
            Ok(Value::float(nums[mid]))
        }
    });

    // list/mode — statistical mode (most frequent)
    register_fn(env, "list/mode", |args| {
        check_arity!(args, "list/mode", 1);
        let items = get_sequence(&args[0], "list/mode")?;
        if items.is_empty() {
            return Err(SemaError::eval("list/mode: empty list"));
        }
        let mut counts: std::collections::BTreeMap<Value, usize> =
            std::collections::BTreeMap::new();
        for item in items {
            *counts.entry(item.clone()).or_insert(0) += 1;
        }
        let max_count = counts.values().copied().max().unwrap();
        let modes: Vec<Value> = counts
            .into_iter()
            .filter(|(_, c)| *c == max_count)
            .map(|(v, _)| v)
            .collect();
        if modes.len() == 1 {
            Ok(modes.into_iter().next().unwrap())
        } else {
            Ok(Value::list(modes))
        }
    });

    // list/diff — set difference
    register_fn(env, "list/diff", |args| {
        check_arity!(args, "list/diff", 2);
        let a = get_sequence(&args[0], "list/diff")?;
        let b = get_sequence(&args[1], "list/diff")?;
        let b_set: std::collections::BTreeSet<Value> = b.iter().cloned().collect();
        let result: Vec<Value> = a
            .iter()
            .filter(|item| !b_set.contains(item))
            .cloned()
            .collect();
        Ok(Value::list(result))
    });

    // list/intersect — set intersection
    register_fn(env, "list/intersect", |args| {
        check_arity!(args, "list/intersect", 2);
        let a = get_sequence(&args[0], "list/intersect")?;
        let b = get_sequence(&args[1], "list/intersect")?;
        let b_set: std::collections::BTreeSet<Value> = b.iter().cloned().collect();
        let result: Vec<Value> = a
            .iter()
            .filter(|item| b_set.contains(item))
            .cloned()
            .collect();
        Ok(Value::list(result))
    });

    // list/sliding — sliding window
    register_fn(env, "list/sliding", |args| {
        check_arity!(args, "list/sliding", 2..=3);
        let items = get_sequence(&args[0], "list/sliding")?;
        let size = args[1].as_index("list/sliding")?;
        let step = if args.len() == 3 {
            args[2].as_index("list/sliding")?
        } else {
            1
        };
        if size == 0 {
            return Err(SemaError::eval("list/sliding: size must be positive"));
        }
        if step == 0 {
            return Err(SemaError::eval("list/sliding: step must be positive"));
        }
        let mut result = Vec::new();
        let mut i = 0;
        while i + size <= items.len() {
            result.push(Value::list(items[i..i + size].to_vec()));
            i += step;
        }
        Ok(Value::list(result))
    });

    // list/key-by — turn list of maps into map keyed by fn
    register_fn(env, "list/key-by", |args| {
        check_arity!(args, "list/key-by", 2);
        let items = get_sequence(&args[1], "list/key-by")?;
        let mut map = std::collections::BTreeMap::new();
        for item in items {
            let key = call_function(&args[0], &[item.clone()])?;
            map.insert(key, item.clone());
        }
        Ok(Value::map(map))
    });

    // list/times — generate list by calling fn N times
    register_fn(env, "list/times", |args| {
        check_arity!(args, "list/times", 2);
        let n = args[0].as_index("list/times")?;
        let mut result = Vec::with_capacity(n);
        for i in 0..n {
            result.push(call_function(&args[1], &[Value::int(i as i64)])?);
        }
        Ok(Value::list(result))
    });

    // list/duplicates — find duplicate values
    register_fn(env, "list/duplicates", |args| {
        check_arity!(args, "list/duplicates", 1);
        let items = get_sequence(&args[0], "list/duplicates")?;
        let mut seen: std::collections::BTreeSet<Value> = std::collections::BTreeSet::new();
        let mut dupes: std::collections::BTreeSet<Value> = std::collections::BTreeSet::new();
        for item in items {
            if !seen.insert(item.clone()) {
                dupes.insert(item.clone());
            }
        }
        Ok(Value::list(dupes.into_iter().collect()))
    });

    // list/cross-join — cartesian product
    register_fn(env, "list/cross-join", |args| {
        check_arity!(args, "list/cross-join", 2);
        let a = get_sequence(&args[0], "list/cross-join")?;
        let b = get_sequence(&args[1], "list/cross-join")?;
        let mut result = Vec::with_capacity(a.len() * b.len());
        for ai in a {
            for bi in b {
                result.push(Value::list(vec![ai.clone(), bi.clone()]));
            }
        }
        Ok(Value::list(result))
    });

    // list/page — pagination
    register_fn(env, "list/page", |args| {
        check_arity!(args, "list/page", 3);
        let items = get_sequence(&args[0], "list/page")?;
        let page = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        let per_page = args[2].as_index("list/page")?;
        if page < 1 {
            return Err(SemaError::eval("list/page: page must be >= 1"));
        }
        let start = ((page - 1) as usize) * per_page;
        if start >= items.len() {
            return Ok(Value::list(vec![]));
        }
        let end = (start + per_page).min(items.len());
        Ok(Value::list(items[start..end].to_vec()))
    });

    // list/find — first matching item
    register_fn(env, "list/find", |args| {
        check_arity!(args, "list/find", 2);
        let items = get_sequence(&args[1], "list/find")?;
        for item in items {
            let result = call_function(&args[0], &[item.clone()])?;
            if result.is_truthy() {
                return Ok(item.clone());
            }
        }
        Ok(Value::nil())
    });

    // list/pad — pad list to length
    register_fn(env, "list/pad", |args| {
        check_arity!(args, "list/pad", 3);
        let mut items = get_sequence(&args[0], "list/pad")?.to_vec();
        let target_len = args[1].as_index("list/pad")?;
        let fill = args[2].clone();
        while items.len() < target_len {
            items.push(fill.clone());
        }
        Ok(Value::list(items))
    });

    // list/sole — single matching item or error
    register_fn(env, "list/sole", |args| {
        check_arity!(args, "list/sole", 2);
        let items = get_sequence(&args[1], "list/sole")?;
        let mut found: Option<Value> = None;
        for item in items {
            let result = call_function(&args[0], &[item.clone()])?;
            if result.is_truthy() {
                if found.is_some() {
                    return Err(SemaError::eval("list/sole: more than one matching item"));
                }
                found = Some(item.clone());
            }
        }
        found.ok_or_else(|| SemaError::eval("list/sole: no matching item"))
    });

    // list/join — join with optional final separator
    register_fn(env, "list/join", |args| {
        check_arity!(args, "list/join", 2..=3);
        let items = get_sequence(&args[0], "list/join")?;
        let sep = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();
        let final_sep = if args.len() == 3 {
            args[2]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[2].type_name()))?
                .to_string()
        } else {
            sep.clone()
        };
        if items.is_empty() {
            return Ok(Value::string(""));
        }
        use std::fmt::Write;
        let mut out = String::with_capacity(items.len().saturating_mul(8));
        let last = items.len().saturating_sub(1);
        for (i, v) in items.iter().enumerate() {
            if i > 0 {
                out.push_str(if i == last { &final_sep } else { &sep });
            }
            write!(&mut out, "{}", v).unwrap();
        }
        Ok(Value::string(&out))
    });

    // tap — side-effect then return original
    register_fn(env, "tap", |args| {
        check_arity!(args, "tap", 2);
        call_function(&args[1], &[args[0].clone()])?;
        Ok(args[0].clone())
    });

    // Car/cdr compositions (2-deep)
    register_fn(env, "caar", |args| first(&[first(args)?]));
    register_fn(env, "cadr", |args| first(&[rest(args)?]));
    register_fn(env, "cdar", |args| rest(&[first(args)?]));
    register_fn(env, "cddr", |args| rest(&[rest(args)?]));

    // Car/cdr compositions (3-deep)
    register_fn(env, "caaar", |args| first(&[first(&[first(args)?])?]));
    register_fn(env, "caadr", |args| first(&[first(&[rest(args)?])?]));
    register_fn(env, "cadar", |args| first(&[rest(&[first(args)?])?]));
    register_fn(env, "caddr", |args| first(&[rest(&[rest(args)?])?]));
    register_fn(env, "cdaar", |args| rest(&[first(&[first(args)?])?]));
    register_fn(env, "cdadr", |args| rest(&[first(&[rest(args)?])?]));
    register_fn(env, "cddar", |args| rest(&[rest(&[first(args)?])?]));
    register_fn(env, "cdddr", |args| rest(&[rest(&[rest(args)?])?]));

    // Association list functions (assoc is dual-purpose in map.rs)
    register_fn(env, "assq", |args| {
        check_arity!(args, "assq", 2);
        let key = &args[0];
        let alist = get_sequence(&args[1], "assq")?;
        for pair in alist {
            if let Some(p) = pair.as_list() {
                if !p.is_empty() && &p[0] == key {
                    return Ok(pair.clone());
                }
            }
        }
        Ok(Value::bool(false))
    });

    register_fn(env, "assv", |args| {
        check_arity!(args, "assv", 2);
        let key = &args[0];
        let alist = get_sequence(&args[1], "assv")?;
        for pair in alist {
            if let Some(p) = pair.as_list() {
                if !p.is_empty() && &p[0] == key {
                    return Ok(pair.clone());
                }
            }
        }
        Ok(Value::bool(false))
    });

    // Silent aliases for other Lisp dialects (undocumented)
    if let Some(v) = env.get(sema_core::intern("map")) {
        env.set(sema_core::intern("mapcar"), v);
    }
    if let Some(v) = env.get(sema_core::intern("foldl")) {
        env.set(sema_core::intern("fold"), v);
    }
    if let Some(v) = env.get(sema_core::intern("any")) {
        env.set(sema_core::intern("some?"), v.clone());
        env.set(sema_core::intern("any?"), v);
    }
    if let Some(v) = env.get(sema_core::intern("every")) {
        env.set(sema_core::intern("every?"), v);
    }
}

fn first(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "car", 1);
    if let Some(l) = args[0].as_list() {
        if l.is_empty() {
            Ok(Value::nil())
        } else {
            Ok(l[0].clone())
        }
    } else if let Some(v) = args[0].as_vector() {
        if v.is_empty() {
            Ok(Value::nil())
        } else {
            Ok(v[0].clone())
        }
    } else {
        Err(SemaError::type_error("list or vector", args[0].type_name())
            .with_hint("first: argument 1 must be a list or vector"))
    }
}

fn rest(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "cdr", 1);
    if let Some(l) = args[0].as_list() {
        if l.len() <= 1 {
            Ok(Value::list(vec![]))
        } else {
            Ok(Value::list(l[1..].to_vec()))
        }
    } else if let Some(v) = args[0].as_vector() {
        if v.len() <= 1 {
            Ok(Value::vector(vec![]))
        } else {
            Ok(Value::vector(v[1..].to_vec()))
        }
    } else {
        Err(SemaError::type_error("list or vector", args[0].type_name())
            .with_hint("rest: argument 1 must be a list or vector"))
    }
}

fn get_sequence<'a>(val: &'a Value, ctx: &str) -> Result<&'a [Value], SemaError> {
    if let Some(l) = val.as_list() {
        Ok(l)
    } else if let Some(v) = val.as_vector() {
        Ok(v)
    } else {
        Err(
            SemaError::type_error("list or vector", format!("{} in {ctx}", val.type_name()))
                .with_hint(format!("{ctx}: expected a list or vector to iterate over")),
        )
    }
}

fn flatten_recursive(val: &Value, out: &mut Vec<Value>) {
    if let Some(l) = val.as_list() {
        for item in l.iter() {
            flatten_recursive(item, out);
        }
    } else if let Some(v) = val.as_vector() {
        for item in v.iter() {
            flatten_recursive(item, out);
        }
    } else {
        out.push(val.clone());
    }
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

/// Call a Sema function (lambda or native) with given args.
/// Delegates to the real evaluator via the registered callback.
///
/// VM closures called from inside an async task route through the scheduler
/// (see `run_closure_as_inline_task` in sema-vm), so yields suspend cleanly.
/// Plain native callbacks (e.g. `(map channel/recv ...)`) don't have that
/// affordance — their yield signal would be silently dropped or coalesced
/// with subsequent calls, producing wrong results. Surface that case as an
/// explicit error pointing to the lambda-wrap workaround.
pub fn call_function(func: &Value, args: &[Value]) -> Result<Value, SemaError> {
    let result = if let Some(native) = func.as_native_fn_rc() {
        sema_core::with_stdlib_ctx(|ctx| (native.func)(ctx, args))
    } else {
        sema_core::with_stdlib_ctx(|ctx| sema_core::call_callback(ctx, func, args))
    };

    if sema_core::in_async_context() && sema_core::take_yield_signal().is_some() {
        return Err(SemaError::eval(
            "yielding native passed directly to a higher-order function — \
             wrap it in a lambda so the yield can suspend cleanly. \
             For example, `(map (fn (x) (channel/recv x)) ...)` instead of \
             `(map channel/recv ...)`.",
        ));
    }

    result
}
