//! Sema-level OpenTelemetry surface. Thin wrappers over the `sema-otel` facade — all
//! no-ops when telemetry is disabled (and on wasm, where the facade compiles out).
//!
//! - Spans: `otel/span`, `otel/llm-span`, `otel/tool-span`, `otel/retrieval-span` (each
//!   runs a thunk inside a typed span, sets Error status if it throws, returns its value).
//! - Annotate the current span: `otel/set-attribute(s)`, `otel/set-status`,
//!   `otel/llm-usage`, `otel/event`.
//! - Grouping: `otel/with-session` (Langfuse sessions / users).
//!
//! Typed spans also emit the `SEMA_OTEL_COMPAT` span-kind, so user-built pipelines render
//! first-class in Phoenix/Traceloop/Langfuse exactly like the built-in `llm/*` spans.

use sema_core::{Env, SemaError, Value};
use sema_otel::AttrValue;

/// `(key . value)` pairs from a Sema map, keyed by keyword or string. Non-map → empty.
fn map_entries(v: &Value) -> Vec<(String, Value)> {
    match v.as_map_rc() {
        Some(m) => m
            .iter()
            .filter_map(|(k, val)| {
                let key = k
                    .as_keyword()
                    .or_else(|| k.as_str().map(|s| s.to_string()))?;
                Some((key, val.clone()))
            })
            .collect(),
        None => Vec::new(),
    }
}

/// Map a Sema value to a typed span-attribute value (bool/int/float preserved; anything
/// else stringified). Order matters: `as_bool` is strict, `as_int` is integers-only, and
/// `as_float` also matches integers — so bool → int → float → string.
fn attr_value(v: &Value) -> AttrValue {
    if let Some(b) = v.as_bool() {
        AttrValue::Bool(b)
    } else if let Some(i) = v.as_int() {
        AttrValue::Int(i)
    } else if let Some(f) = v.as_float() {
        AttrValue::Float(f)
    } else if let Some(s) = v.as_str() {
        AttrValue::Str(s.to_string())
    } else {
        AttrValue::Str(v.to_string())
    }
}

/// Parse a Sema attrs-map into typed `(key, AttrValue)` pairs.
fn parse_attrs(v: Option<&Value>) -> Vec<(String, AttrValue)> {
    match v {
        Some(m) => map_entries(m)
            .into_iter()
            .map(|(k, val)| (k, attr_value(&val)))
            .collect(),
        None => Vec::new(),
    }
}

/// A keyword/string argument as a plain `String` (for keys, status, session ids).
fn as_name(v: &Value) -> Option<String> {
    v.as_keyword().or_else(|| v.as_str().map(|s| s.to_string()))
}

/// Run `thunk` inside `span`, setting Error status if it returns an error, then end the
/// span (on drop). Shared by every typed-span builtin.
fn run_in_span(span: sema_otel::VmSpan, thunk: &Value) -> Result<Value, SemaError> {
    let result = crate::list::call_function(thunk, &[]);
    if let Err(e) = &result {
        sema_otel::set_current_status(Some(&e.to_string()));
    }
    drop(span);
    result
}

pub fn register(env: &Env) {
    // (otel/span name thunk) / (otel/span name thunk attrs) — generic INTERNAL span.
    crate::register_fn(env, "otel/span", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("otel/span", "2-3", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let span = sema_otel::user_span(
            name,
            sema_otel::SemaSpanKind::Internal,
            parse_attrs(args.get(2)),
        );
        run_in_span(span, &args[1])
    });

    // (otel/llm-span config-map thunk) — typed LLM/generation span. config: :model
    // :provider :operation (+ any extra attrs, passed through).
    crate::register_fn(env, "otel/llm-span", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("otel/llm-span", "2", args.len()));
        }
        let (mut model, mut provider, mut operation) =
            (String::new(), String::new(), String::new());
        let mut attrs = Vec::new();
        for (k, val) in map_entries(&args[0]) {
            match k.as_str() {
                "model" => model = val.as_str().map(|s| s.to_string()).unwrap_or_default(),
                "provider" => provider = val.as_str().map(|s| s.to_string()).unwrap_or_default(),
                "operation" => operation = val.as_str().map(|s| s.to_string()).unwrap_or_default(),
                _ => attrs.push((k, attr_value(&val))),
            }
        }
        let span = sema_otel::user_llm_span(&model, &provider, &operation, attrs);
        run_in_span(span, &args[1])
    });

    // (otel/tool-span name thunk) / (... attrs) — typed TOOL span.
    crate::register_fn(env, "otel/tool-span", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("otel/tool-span", "2-3", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let span = sema_otel::user_span(
            name,
            sema_otel::SemaSpanKind::Tool,
            parse_attrs(args.get(2)),
        );
        run_in_span(span, &args[1])
    });

    // (otel/retrieval-span name thunk) / (... attrs) — typed RETRIEVER span.
    crate::register_fn(env, "otel/retrieval-span", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("otel/retrieval-span", "2-3", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let span = sema_otel::user_span(
            name,
            sema_otel::SemaSpanKind::Retrieval,
            parse_attrs(args.get(2)),
        );
        run_in_span(span, &args[1])
    });

    // (otel/set-attribute key value) — set one attribute on the innermost active span.
    crate::register_fn(env, "otel/set-attribute", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("otel/set-attribute", "2", args.len()));
        }
        let key = as_name(&args[0])
            .ok_or_else(|| SemaError::type_error("keyword or string", args[0].type_name()))?;
        sema_otel::set_current_attr(&key, attr_value(&args[1]));
        Ok(Value::nil())
    });

    // (otel/set-attributes {:k v ...}) — set many attributes on the innermost span.
    crate::register_fn(env, "otel/set-attributes", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("otel/set-attributes", "1", args.len()));
        }
        sema_otel::set_current_attrs(parse_attrs(Some(&args[0])));
        Ok(Value::nil())
    });

    // (otel/set-status :ok) / (otel/set-status :error "msg") — status on the innermost span.
    crate::register_fn(env, "otel/set-status", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("otel/set-status", "1-2", args.len()));
        }
        let status = as_name(&args[0])
            .ok_or_else(|| SemaError::type_error("keyword or string", args[0].type_name()))?;
        if status == "error" {
            let msg = args.get(1).and_then(|v| v.as_str()).unwrap_or("error");
            sema_otel::set_current_status(Some(msg));
        } else {
            sema_otel::set_current_status(None);
        }
        Ok(Value::nil())
    });

    // (otel/llm-usage {:input-tokens N :output-tokens N :cost-usd F}) — usage on the
    // innermost span (typically inside an otel/llm-span).
    crate::register_fn(env, "otel/llm-usage", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("otel/llm-usage", "1", args.len()));
        }
        let (mut input, mut output, mut cost) = (0u32, 0u32, None);
        for (k, val) in map_entries(&args[0]) {
            match k.as_str() {
                "input-tokens" => input = val.as_int().unwrap_or(0).max(0) as u32,
                "output-tokens" => output = val.as_int().unwrap_or(0).max(0) as u32,
                "cost-usd" => cost = val.as_float(),
                _ => {}
            }
        }
        sema_otel::set_current_llm_usage(input, output, cost);
        Ok(Value::nil())
    });

    // (otel/with-session id thunk) / (otel/with-session id {:user "..."} thunk) — group
    // the spans started in `thunk` into a session (Langfuse Sessions/Users).
    crate::register_fn(env, "otel/with-session", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("otel/with-session", "2-3", args.len()));
        }
        let session = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let (user, thunk) = if args.len() == 3 {
            let user = map_entries(&args[1])
                .into_iter()
                .find(|(k, _)| k == "user")
                .and_then(|(_, v)| v.as_str().map(|s| s.to_string()));
            (user, &args[2])
        } else {
            (None, &args[1])
        };
        let guard = sema_otel::set_conversation_scope(session, Some(session), user.as_deref());
        let result = crate::list::call_function(thunk, &[]);
        drop(guard);
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

    // (otel/configure {:endpoint "..." :key "..." ...}) — point Sema at a tracing backend
    // from code instead of environment variables. Installs a provider on the first call;
    // returns true if telemetry was turned on by this call, false if nothing was
    // configured or telemetry was already active (env at startup / an earlier configure).
    //
    // Keys: :endpoint (OTLP url) · :file (JSONL path) · :protocol (http/protobuf|http/json|
    // grpc) · :key (API key → `Authorization: Bearer <key>`) · :headers (a map of extra
    // headers, or a pre-formatted "name=value,..." string) · :service-name · :environment ·
    // :release · :capture-content (bool). Setting :endpoint or :file turns tracing on.
    crate::register_fn(env, "otel/configure", |args| {
        if args.len() != 1 {
            return Err(SemaError::arity("otel/configure", "1", args.len()));
        }
        if args[0].as_map_rc().is_none() {
            return Err(SemaError::type_error("map", args[0].type_name()));
        }
        let mut cfg = sema_otel::OtelConfig::default();
        // Header pairs accumulate from :key and a :headers map; a :headers string is kept
        // verbatim. All are joined into the comma-separated OTLP header format at the end.
        let mut header_pairs: Vec<(String, String)> = Vec::new();
        let mut header_str: Option<String> = None;

        for (k, val) in map_entries(&args[0]) {
            match k.as_str() {
                "endpoint" => cfg.endpoint = as_name(&val),
                "file" => cfg.file = val.as_str().map(|s| s.to_string()),
                "protocol" => cfg.protocol = as_name(&val),
                "service-name" | "service" => cfg.service_name = as_name(&val),
                "environment" | "env" => cfg.environment = as_name(&val),
                "release" => cfg.release = as_name(&val),
                "capture-content" => cfg.capture_content = Some(val.as_bool().unwrap_or(false)),
                // Shorthand: an API key becomes a Bearer auth header.
                "key" => {
                    if let Some(s) = val.as_str() {
                        header_pairs.push(("Authorization".to_string(), format!("Bearer {s}")));
                    }
                }
                "headers" => {
                    if let Some(s) = val.as_str() {
                        header_str = Some(s.to_string());
                    } else {
                        for (hk, hv) in map_entries(&val) {
                            let hvs = hv
                                .as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| hv.to_string());
                            header_pairs.push((hk, hvs));
                        }
                    }
                }
                _ => {}
            }
        }

        let mut parts: Vec<String> = header_pairs
            .into_iter()
            .map(|(n, v)| format!("{n}={v}"))
            .collect();
        if let Some(s) = header_str {
            parts.push(s);
        }
        if !parts.is_empty() {
            cfg.headers = Some(parts.join(","));
        }

        Ok(Value::bool(sema_otel::configure(&cfg)))
    });
}
