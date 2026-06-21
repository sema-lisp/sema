//! Sema-level OpenTelemetry surface: `(otel/span name thunk)` and
//! `(otel/event name attrs-map)`. Thin wrappers over the `sema-otel` facade — no-ops
//! when telemetry is disabled (and on wasm, where the facade compiles out).

use sema_core::{Env, SemaError, Value};

pub fn register(env: &Env) {
    // (otel/span "name" thunk) — run thunk inside a named INTERNAL span; returns the
    // thunk's value. The span ends after the thunk completes (recording duration).
    crate::register_fn(env, "otel/span", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("otel/span", "2", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let span = sema_otel::vm_span(name);
        let result = crate::list::call_function(&args[1], &[]);
        drop(span);
        result
    });

    // (otel/event "name") / (otel/event "name" {:k "v" ...}) — add an event to the
    // current span. Attribute values are stringified. Returns nil.
    crate::register_fn(env, "otel/event", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("otel/event", "1-2", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let attrs: Vec<(String, String)> = match args.get(1).and_then(|v| v.as_map_rc()) {
            Some(m) => m
                .iter()
                .filter_map(|(k, v)| {
                    let key = k
                        .as_keyword()
                        .or_else(|| k.as_str().map(|s| s.to_string()))?;
                    let val = v
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| v.to_string());
                    Some((key, val))
                })
                .collect(),
            None => Vec::new(),
        };
        sema_otel::add_event(name, attrs);
        Ok(Value::nil())
    });
}
