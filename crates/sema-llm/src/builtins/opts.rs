use super::*;

/// Parse one `llm/with-fallback` chain element into a [`FallbackEntry`].
///
/// Accepted shapes:
/// - `:provider` / `"provider"` — bare name, uses the provider's default model
/// - `[:provider "model"]` — pair, with a per-provider model override
/// - `{:provider :name :model "model"}` — map form, `:model` optional
pub(super) fn parse_fallback_entry(v: &Value) -> Result<FallbackEntry, SemaError> {
    // Bare keyword or string.
    if let Some(name) = v.as_keyword().or_else(|| v.as_str().map(|s| s.to_string())) {
        return Ok(FallbackEntry {
            provider: name,
            model: None,
        });
    }
    // Map form: {:provider .. :model ..}. The :provider value may be a keyword or
    // a string.
    if let Some(map) = v.as_map_ref() {
        let provider = map
            .get(&Value::keyword("provider"))
            .and_then(|p| p.as_keyword().or_else(|| p.as_str().map(|s| s.to_string())))
            .ok_or_else(|| {
                SemaError::eval("fallback map entry must have a :provider key (keyword or string)")
            })?;
        return Ok(FallbackEntry {
            provider,
            model: map.opt_str("model"),
        });
    }
    // Pair form: [:provider "model"].
    if let Some(seq) = v.as_seq() {
        if seq.len() != 2 {
            return Err(SemaError::eval(
                "fallback pair entry must be [provider model]",
            ));
        }
        let provider = seq[0]
            .as_keyword()
            .or_else(|| seq[0].as_str().map(|s| s.to_string()))
            .ok_or_else(|| SemaError::type_error("keyword or string", seq[0].type_name()))?;
        let model = seq[1]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| SemaError::type_error("string model", seq[1].type_name()))?;
        return Ok(FallbackEntry {
            provider,
            model: Some(model),
        });
    }
    Err(SemaError::type_error(
        "keyword, string, [provider model] pair, or map",
        v.type_name(),
    ))
}

/// Read an optional per-call `:timeout` (milliseconds) from a call's options argument.
pub(super) fn opt_timeout_ms(opts_arg: Option<&Value>) -> Option<u64> {
    opts_arg
        .and_then(|v| v.opt_int("timeout"))
        .map(|n| n as u32 as u64)
}

/// Read an optional list-of-strings option for observability tags: `:tags ["a" "b"]`,
/// or a lone string `:tags "a"`. Non-string elements are skipped.
pub(super) fn get_opt_string_list(opts: &BTreeMap<Value, Value>, key: &str) -> Vec<String> {
    match opts.get(&Value::keyword(key)) {
        Some(v) if v.as_seq().is_some() => v
            .as_seq()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()).or_else(|| x.as_keyword()))
            .collect(),
        Some(v) => v
            .as_str()
            .map(|s| vec![s.to_string()])
            .or_else(|| v.as_keyword().map(|s| vec![s]))
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Read an optional `string -> string` map option for observability metadata:
/// `:metadata {:env "prod" :team "ml"}`. Keyword keys are de-coloned (`:env` -> `env`);
/// values are stringified.
pub(super) fn get_opt_str_map(opts: &BTreeMap<Value, Value>, key: &str) -> Vec<(String, String)> {
    let Some(m) = opts.get(&Value::keyword(key)).and_then(|v| v.as_map_rc()) else {
        return Vec::new();
    };
    m.iter()
        .map(|(k, val)| {
            let ks = k
                .as_keyword()
                .or_else(|| k.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| k.to_string());
            let vs = val
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| val.to_string());
            (ks, vs)
        })
        .collect()
}
