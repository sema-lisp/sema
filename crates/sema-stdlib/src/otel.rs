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

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    NativeCall, NativeCallContext, NativeContinuation, NativeOutcome, NativeResult, ResumeInput,
    Trace,
};
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
/// span (on drop). Shared by every typed-span builtin's LEGACY (bare top-level / legacy
/// scheduler) path.
fn run_in_span(span: sema_otel::VmSpan, thunk: &Value) -> Result<Value, SemaError> {
    let result = crate::list::call_function(thunk, &[]);
    if let Err(e) = &result {
        sema_otel::set_current_status(Some(&e.to_string()));
    }
    drop(span);
    result
}

/// Cooperative teardown for a typed-span builtin (`otel/span`, `otel/llm-span`,
/// `otel/tool-span`, `otel/retrieval-span`) under the unified runtime. The builtin
/// opens the span (pushing it onto the TL span stack) and hands this continuation
/// the guard; the runtime drives the wrapped thunk as a `NativeOutcome::Call`, so an
/// async op inside it (`async/spawn`, `channel/*`, …) parks on the active task
/// instead of hitting the runtime-only error stub a synchronous `call_function`
/// re-entry would. When the thunk settles the span is still the innermost active
/// span, so a failure sets Error status on it exactly like `run_in_span`; the span is
/// then ended (dropped) on return, failure, AND cancellation, and the original
/// outcome is re-propagated so an enclosing try/catch sees the same value/error as the
/// synchronous path. `VmSpan` owns only OTel context state (no `Value`), so this
/// continuation exposes no GC edges.
struct SpanGuardContinuation {
    span: Option<sema_otel::VmSpan>,
}

impl Trace for SpanGuardContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for SpanGuardContinuation {
    fn resume(
        mut self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let span = self.span.take();
        match input {
            ResumeInput::Returned(value) => {
                drop(span);
                Ok(NativeOutcome::Return(value))
            }
            ResumeInput::Failed(error) => {
                sema_otel::set_current_status(Some(&error.to_string()));
                drop(span);
                Err(error)
            }
            ResumeInput::Cancelled(reason) => {
                sema_otel::set_current_status(Some(&format!("cancelled ({reason:?})")));
                drop(span);
                Err(SemaError::eval(format!(
                    "otel span thunk was cancelled ({reason:?})"
                )))
            }
            ResumeInput::Runtime(_) => {
                drop(span);
                Err(SemaError::eval(
                    "otel span teardown received an unexpected runtime response",
                ))
            }
        }
    }
}

/// Register a typed-span builtin as a DUAL-ABI native. `setup` validates the args and
/// OPENS the span (returning it plus the body thunk). Under a runtime quantum the VM
/// invokes the runtime callback, which drives the thunk as a cooperative
/// `NativeOutcome::Call` with `SpanGuardContinuation` closing the span when it settles;
/// everywhere else the legacy callback runs the thunk synchronously inside `run_in_span`.
fn register_span_fn(
    env: &Env,
    name: &'static str,
    setup: impl Fn(&[Value]) -> Result<(sema_otel::VmSpan, Value), SemaError> + 'static,
) {
    let setup = std::rc::Rc::new(setup);
    let for_legacy = setup.clone();
    let for_runtime = setup;
    env.set(
        sema_core::intern(name),
        Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            name,
            move |args| {
                let (span, thunk) = for_legacy(args)?;
                run_in_span(span, &thunk)
            },
            move |_ctx, args| {
                let (span, thunk) = for_runtime(args)?;
                Ok(NativeOutcome::Call(NativeCall {
                    callable: thunk,
                    args: Vec::new(),
                    continuation: Box::new(SpanGuardContinuation { span: Some(span) }),
                }))
            },
        )),
    );
}

pub fn register(env: &Env) {
    // (otel/span name thunk) / (otel/span name thunk attrs) — generic INTERNAL span.
    register_span_fn(env, "otel/span", |args| {
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
        Ok((span, args[1].clone()))
    });

    // (otel/llm-span config-map thunk) — typed LLM/generation span. config: :model
    // :provider :operation (+ any extra attrs, passed through).
    register_span_fn(env, "otel/llm-span", |args| {
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
        Ok((span, args[1].clone()))
    });

    // (otel/tool-span name thunk) / (... attrs) — typed TOOL span.
    register_span_fn(env, "otel/tool-span", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("otel/tool-span", "2-3", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let span = sema_otel::user_span(name, sema_otel::SemaSpanKind::Tool, parse_attrs(args.get(2)));
        Ok((span, args[1].clone()))
    });

    // (otel/retrieval-span name thunk) / (... attrs) — typed RETRIEVER span.
    register_span_fn(env, "otel/retrieval-span", |args| {
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
        Ok((span, args[1].clone()))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The typed-span teardown continuation holds only a `VmSpan` (OTel context
    /// state — no `Value`), so it exposes ZERO GC edges: the runtime traces it
    /// without visiting any `Value`.
    #[test]
    fn span_guard_continuation_holds_no_gc_edges() {
        let cont = SpanGuardContinuation { span: None };
        let mut edges = 0usize;
        assert!(cont.trace(&mut |_| edges += 1));
        assert_eq!(edges, 0, "span teardown continuation must expose no Value edges");
    }
}
