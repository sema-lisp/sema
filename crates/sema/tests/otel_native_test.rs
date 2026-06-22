//! Sema-native tracing surface: the `otel/*` builtins + `with-span`/`with-session`
//! macros emit typed spans, attributes, status, usage, and session grouping — rendering
//! like the built-in `llm/*` spans (compat aliases included). Deterministic (in-memory
//! exporter, no network). Own binary (the global provider + compat override are
//! process-global).

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;

#[test]
fn native_tracing_surface_emits_typed_spans() {
    sema_otel::testing::set_compat("all");
    let cap = sema_otel::testing::install();
    let interp = Interpreter::new();

    let src = r#"
        (with-span "ingest" {:batch.size 100 :flag true}
          (otel/set-attribute :rows 42)
          (otel/set-status :ok))

        (otel/llm-span {:model "custom-model" :provider "anthropic" :operation "chat"}
          (lambda ()
            (otel/llm-usage {:input-tokens 120 :output-tokens 30 :cost-usd 0.001})
            "ok"))

        (otel/tool-span "lookup-weather" (lambda () "sunny"))

        (otel/retrieval-span "vector-search" (lambda () "docs") {:top-k 5})

        (with-session "sess-1" {:user "alice"}
          (with-span "inner" {} nil))

        (try (with-span "boom" {} (throw "kaboom"))
          (catch e nil))
    "#;
    interp
        .eval_str_compiled(src)
        .expect("eval native otel surface");

    let span = |name: &str| {
        cap.span_named(name)
            .unwrap_or_else(|| panic!("missing span {name}"))
    };
    let a = |name: &str, key: &str| span(name)["attributes"][key].clone();

    // Generic span: compat CHAIN kind, typed attrs from the map, and a runtime
    // otel/set-attribute all land on the same span.
    assert_eq!(a("ingest", "openinference.span.kind"), "CHAIN");
    assert_eq!(a("ingest", "batch.size"), 100);
    assert_eq!(a("ingest", "flag"), true);
    assert_eq!(a("ingest", "rows"), 42);

    // Typed LLM span: gen_ai.* request + usage identical to the built-in path, plus the
    // OpenInference compat aliases — a custom-provider call accounts the same.
    assert_eq!(a("chat", "openinference.span.kind"), "LLM");
    assert_eq!(a("chat", "gen_ai.request.model"), "custom-model");
    assert_eq!(a("chat", "gen_ai.provider.name"), "anthropic");
    assert_eq!(a("chat", "gen_ai.usage.input_tokens"), 120);
    assert_eq!(a("chat", "gen_ai.usage.total_tokens"), 150);
    assert_eq!(a("chat", "llm.token_count.prompt"), 120);
    assert!(
        (a("chat", "gen_ai.usage.cost").as_f64().unwrap() - 0.001).abs() < 1e-9,
        "gen_ai.usage.cost"
    );

    // Typed tool + retrieval spans classify correctly.
    assert_eq!(a("lookup-weather", "openinference.span.kind"), "TOOL");
    assert_eq!(a("lookup-weather", "gen_ai.operation.name"), "execute_tool");
    assert_eq!(a("lookup-weather", "gen_ai.tool.name"), "lookup-weather");
    assert_eq!(a("vector-search", "openinference.span.kind"), "RETRIEVER");
    assert_eq!(a("vector-search", "top-k"), 5);

    // Session scope flows onto every span started inside the block.
    assert_eq!(a("inner", "session.id"), "sess-1");
    assert_eq!(a("inner", "user.id"), "alice");

    // A throwing body marks the span's status as error (and the value is still returned
    // by the surrounding try/catch — tracing never changes program semantics).
    assert_eq!(a("boom", "error.type"), "error");
}
