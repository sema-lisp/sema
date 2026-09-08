use sema_core::{intern, resolve, SemaError, Spur, Value};

pub type Bindings = Vec<(Spur, Value)>;

/// Destructure a pattern against a value, producing bindings.
/// Errors on shape mismatch (for use in `let`/`define`/`lambda`).
pub fn destructure(pattern: &Value, value: &Value) -> Result<Bindings, SemaError> {
    let mut out = Vec::new();
    destructure_into(&mut out, pattern, value)?;
    Ok(out)
}

/// Try to match a pattern against a value (for use in `match`).
/// Returns `None` if the pattern doesn't match, `Some(bindings)` if it does.
pub fn try_match(pattern: &Value, value: &Value) -> Result<Option<Bindings>, SemaError> {
    let mut out = Vec::new();
    if match_into(&mut out, pattern, value)? {
        Ok(Some(out))
    } else {
        Ok(None)
    }
}

// ── Destructure (strict) ────────────────────────────────────────────

fn destructure_into(out: &mut Bindings, pattern: &Value, value: &Value) -> Result<(), SemaError> {
    let underscore = intern("_");

    if let Some(spur) = pattern.as_symbol_spur() {
        if spur != underscore {
            out.push((spur, value.clone()));
        }
        return Ok(());
    }

    if let Some(elems) = pattern.as_vector() {
        return destructure_seq(out, elems, value);
    }

    if pattern.as_map_ref().is_some() {
        return destructure_map(out, pattern, value);
    }

    Err(SemaError::eval(format!(
        "destructure: invalid pattern, expected symbol, vector, or map, got {}",
        pattern.type_name()
    ))
    .with_hint("use [a b c] for sequential destructuring or {:keys [a b]} for map destructuring"))
}

fn destructure_seq(out: &mut Bindings, elems: &[Value], value: &Value) -> Result<(), SemaError> {
    let items = if let Some(l) = value.as_list() {
        l
    } else if let Some(v) = value.as_vector() {
        v
    } else {
        return Err(SemaError::type_error("list or vector", value.type_name()));
    };

    let amp = intern("&");

    // Find `&` position
    let amp_pos = elems.iter().position(|e| e.as_symbol_spur() == Some(amp));

    if let Some(pos) = amp_pos {
        // [a b & rest]
        let fixed = &elems[..pos];
        if pos + 1 >= elems.len() {
            return Err(SemaError::eval(
                "destructure: `&` must be followed by a rest pattern",
            ));
        }
        let rest_pattern = &elems[pos + 1];
        if pos + 2 < elems.len() {
            return Err(SemaError::eval(
                "destructure: only one pattern allowed after `&`",
            ));
        }

        if items.len() < fixed.len() {
            return Err(SemaError::eval(format!(
                "destructure: expected at least {} elements before `&`, got {}",
                fixed.len(),
                items.len()
            )));
        }

        for (pat, val) in fixed.iter().zip(items.iter()) {
            destructure_into(out, pat, val)?;
        }

        let rest = Value::list(items[fixed.len()..].to_vec());
        destructure_into(out, rest_pattern, &rest)?;
    } else {
        // [a b c] — exact match
        if items.len() != elems.len() {
            return Err(SemaError::eval(format!(
                "destructure: expected {} elements, got {}",
                elems.len(),
                items.len()
            )));
        }
        for (pat, val) in elems.iter().zip(items.iter()) {
            destructure_into(out, pat, val)?;
        }
    }

    Ok(())
}

fn destructure_map(out: &mut Bindings, pattern: &Value, value: &Value) -> Result<(), SemaError> {
    let map = pattern
        .as_map_ref()
        .ok_or_else(|| SemaError::eval("destructure: expected map pattern"))?;

    if !is_map_value(value) {
        return Err(SemaError::type_error("map or hashmap", value.type_name()));
    }

    let keys_kw = Value::keyword("keys");

    if let Some(keys_val) = map.get(&keys_kw) {
        // {:keys [a b c]} destructuring
        let key_names = if let Some(v) = keys_val.as_vector() {
            v.to_vec()
        } else if let Some(l) = keys_val.as_list() {
            l.to_vec()
        } else {
            return Err(SemaError::eval(
                "destructure: :keys must be followed by a vector or list of symbols",
            ));
        };

        for key_sym in &key_names {
            let spur = key_sym
                .as_symbol_spur()
                .ok_or_else(|| SemaError::eval("destructure: :keys entries must be symbols"))?;
            let kw = Value::keyword(&resolve(spur));
            let val = map_get(value, &kw).unwrap_or(Value::nil());
            out.push((spur, val));
        }
    }

    // Support explicit key-pattern pairs: {:key pattern}
    for (k, v_pat) in map.iter() {
        if k == &keys_kw {
            continue;
        }
        let val = map_get(value, k).unwrap_or(Value::nil());
        destructure_into(out, v_pat, &val)?;
    }

    Ok(())
}

// ── Pattern Match (soft) ────────────────────────────────────────────

fn match_into(out: &mut Bindings, pattern: &Value, value: &Value) -> Result<bool, SemaError> {
    let underscore = intern("_");

    // Wildcard
    if let Some(spur) = pattern.as_symbol_spur() {
        if spur == underscore {
            return Ok(true);
        }
        // Symbol binds
        out.push((spur, value.clone()));
        return Ok(true);
    }

    // Vector pattern: match against list or vector
    if let Some(elems) = pattern.as_vector() {
        return match_seq(out, elems, value);
    }

    // Map pattern: structural match
    if let Some(pat_map) = pattern.as_map_ref() {
        return match_map(out, pat_map, value);
    }

    // List pattern with two elements where first is 'quote': match literal
    if let Some(items) = pattern.as_list() {
        if items.len() == 2 {
            if let Some(s) = items[0].as_symbol_spur() {
                if s == intern("quote") {
                    return Ok(&items[1] == value);
                }
            }
        }
    }

    // Literal match: int, float, string, keyword, bool, char, nil
    Ok(pattern == value)
}

fn match_seq(out: &mut Bindings, elems: &[Value], value: &Value) -> Result<bool, SemaError> {
    let items = if let Some(l) = value.as_list() {
        l
    } else if let Some(v) = value.as_vector() {
        v
    } else {
        return Ok(false);
    };

    let amp = intern("&");
    let amp_pos = elems.iter().position(|e| e.as_symbol_spur() == Some(amp));

    if let Some(pos) = amp_pos {
        let fixed = &elems[..pos];
        if pos + 1 >= elems.len() {
            return Err(SemaError::eval(
                "match: `&` must be followed by a rest pattern",
            ));
        }
        let rest_pattern = &elems[pos + 1];
        if pos + 2 < elems.len() {
            return Err(SemaError::eval("match: only one pattern allowed after `&`"));
        }

        if items.len() < fixed.len() {
            return Ok(false);
        }

        let checkpoint = out.len();
        for (pat, val) in fixed.iter().zip(items.iter()) {
            if !match_into(out, pat, val)? {
                out.truncate(checkpoint);
                return Ok(false);
            }
        }

        let rest = Value::list(items[fixed.len()..].to_vec());
        if !match_into(out, rest_pattern, &rest)? {
            out.truncate(checkpoint);
            return Ok(false);
        }

        Ok(true)
    } else {
        if items.len() != elems.len() {
            return Ok(false);
        }

        let checkpoint = out.len();
        for (pat, val) in elems.iter().zip(items.iter()) {
            if !match_into(out, pat, val)? {
                out.truncate(checkpoint);
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn match_map(
    out: &mut Bindings,
    pat_map: &std::collections::BTreeMap<Value, Value>,
    value: &Value,
) -> Result<bool, SemaError> {
    if !is_map_value(value) {
        return Ok(false);
    }

    let keys_kw = Value::keyword("keys");
    let checkpoint = out.len();

    // Handle {:keys [a b]} in match context — binds symbols from keyword keys
    if let Some(keys_val) = pat_map.get(&keys_kw) {
        let key_names = if let Some(v) = keys_val.as_vector() {
            v.to_vec()
        } else if let Some(l) = keys_val.as_list() {
            l.to_vec()
        } else {
            return Err(SemaError::eval(
                "match: :keys must be followed by a vector or list of symbols",
            ));
        };

        for key_sym in &key_names {
            let spur = key_sym
                .as_symbol_spur()
                .ok_or_else(|| SemaError::eval("match: :keys entries must be symbols"))?;
            let kw = Value::keyword(&resolve(spur));
            let val = map_get(value, &kw).unwrap_or(Value::nil());
            out.push((spur, val));
        }
    }

    // Structural match on explicit key-value patterns
    for (k, v_pat) in pat_map.iter() {
        if k == &keys_kw {
            continue;
        }
        match map_get(value, k) {
            Some(val) => {
                if !match_into(out, v_pat, &val)? {
                    out.truncate(checkpoint);
                    return Ok(false);
                }
            }
            None => {
                out.truncate(checkpoint);
                return Ok(false);
            }
        }
    }

    Ok(true)
}

// ── Helper: uniform map key lookup ──────────────────────────────────

fn map_get(value: &Value, key: &Value) -> Option<Value> {
    if let Some(m) = value.as_map_ref() {
        m.get(key).cloned()
    } else if let Some(m) = value.as_hashmap_ref() {
        m.get(key).cloned()
    } else {
        None
    }
}

fn is_map_value(value: &Value) -> bool {
    value.as_map_ref().is_some() || value.as_hashmap_ref().is_some()
}
