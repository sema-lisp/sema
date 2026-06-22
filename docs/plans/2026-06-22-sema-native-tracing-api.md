# Scoping: Sema-Native Tracing API (emit spans/traces from Sema code)

**Status:** Scoping / design exploration. **NOT for implementation yet.**
**Date:** 2026-06-22
**Context:** The OTel feature (crates/sema-otel) auto-instruments all `llm/*` + `agent/*`
paths with zero user effort. This doc scopes whether/how to also let **Sema programs
emit their own spans/attributes/events** so user-built abstractions (custom HTTP LLM
calls, RAG pipelines, tool orchestration, batch jobs) are traced as first-class
citizens — still "best in class without thinking about it."

---

## 0. What exists today

Two builtins (registered in `sema-stdlib/src/otel.rs`, no-op when telemetry is off):

- `(otel/span "name" thunk)` — run `thunk` inside an INTERNAL span; returns its value.
- `(otel/event "name" attrs-map)` — add an event to the current span.

Limitations:
- **No way to set attributes** on a span from Sema (only events).
- **No typed spans** — everything is INTERNAL, so a user's custom LLM call renders as a
  generic span (not a GENERATION/LLM span) in Langfuse/Phoenix.
- **No way to set input/output, status/error, or span kind.**
- **No way to scope session/user/conversation** around arbitrary Sema code (only
  `agent/run`/`llm/complete` open scopes today).
- **No score/evaluation emission** (a first-class Langfuse/Braintrust concept).
- `(fn () ...)` thunk boilerplate is clunky; a macro would read better.

---

## 1. Design principles

1. **No-op when disabled, zero overhead.** Every form already compiles to nothing when
   no provider is installed; keep that.
2. **Scoped/RAII over manual start/end.** Sema is GC'd; a manual `(span-start)` /
   `(span-end)` pair invites leaks and unbalanced stacks. Prefer block-scoped forms
   that close the span on exit (normal OR error) — mirrors the Rust facade's RAII.
3. **Ergonomic macros over thunks.** `(with-span "name" {...} body...)` beats
   `(otel/span "name" (fn () ...))`.
4. **Typed helpers for the things platforms render specially** (LLM/generation, tool,
   retriever, agent/chain) so custom integrations look first-class — but keep them thin
   over the generic span.
5. **Match the runtime's own attribute vocabulary** (`gen_ai.*`) so user spans and
   auto-spans are indistinguishable to a backend.
6. **Never let a tracing call change program semantics** (a disabled or failing tracer
   must not alter the return value or throw).

---

## 2. Proposed surface (for discussion)

### 2.1 Core: scoped spans with attributes

```sema
;; Macro form (preferred ergonomics) — body is the span scope.
(with-span "ingest-batch" {:batch.size 100 :kind :internal}
  (process-batch))

;; Builtin the macro expands to (thunk form, already exists, extended with attrs):
(otel/span "ingest-batch" (fn () (process-batch)) {:batch.size 100})
```

- Attributes map: keyword/string keys → string/number/bool/array values.
- `:kind` ∈ `:internal` (default) `:client` `:server` `:producer` `:consumer`.
- Returns the body's value; records duration; sets Error status if the body throws.

### 2.2 Set attributes / status on the current span

```sema
(otel/set-attribute :http.status 200)         ; on the innermost active span
(otel/set-attributes {:rows 42 :cache.hit true})
(otel/set-status :error "upstream timeout")    ; or :ok
```

### 2.3 Typed span helpers (render richly in Langfuse/Phoenix)

For users who call an LLM/tool/retriever **themselves** (not via `llm/*`) and want it
traced like the built-ins:

```sema
;; A user-built LLM call (e.g. a provider we don't support natively):
(otel/llm-span {:model "custom-model" :provider "myco" :operation "chat"}
  (fn ()
    (let ((resp (my-http-llm-call prompt)))
      (otel/llm-usage {:input-tokens 120 :output-tokens 30 :cost-usd 0.001})
      resp)))

;; A user-built retrieval step (RAG) — first-class RETRIEVER/retrieval span:
(otel/retrieval-span "vector-search" {:top-k 5}
  (fn () (search index query)))

;; A user tool:
(otel/tool-span "lookup-weather" {:call-id "..."} (fn () (weather city)))
```

These set the right `gen_ai.operation.name` + attributes so the span is typed
correctly (GENERATION / TOOL / retrieval) by `gen_ai.*`-aware backends.

### 2.4 Session / user / conversation scoping from Sema

Expose the existing Rust scope mechanism so non-agent Sema code can group into
sessions/users (Langfuse Sessions/Users):

```sema
(otel/with-session "chat-42" {:user "alice"}
  (fn ()
    (llm/complete "...")      ; inherits session chat-42, user alice
    (my-custom-pipeline)))
```

### 2.5 Events + scores

```sema
(otel/event "cache-miss" {:key "user:42"})                 ; already exists
(otel/score "relevance" 0.92 {:comment "graded by judge"}) ; NEW — Langfuse/Braintrust score
```

`otel/score` maps to the platform's evaluation/score concept (exact OTLP key TBD from
the competitive research — Langfuse ingests scores via its API/`langfuse.score.*`; may
need a span-attribute or a dedicated export path).

---

## 3. Open questions (decide before building)

1. **Macros vs builtins.** Ship `with-span`/`with-session` as prelude macros (nicer) on
   top of thunk builtins, or builtins only? (Recommend: builtins + thin prelude macros.)
2. **Manual start/end at all?** Some users want to start a span in one function and end
   it in another (e.g. request lifecycle across callbacks). Risky in a GC'd lang.
   Options: (a) scoped-only (safest); (b) a handle object with an explicit `(otel/end h)`
   + a finalizer backstop; (c) defer. (Recommend: scoped-only for v1; revisit handles.)
3. **Typed-span vocabulary.** Which typed helpers earn their keep? LLM/generation +
   tool + retrieval are clearly valuable (RAG). Agent/chain/reranker/guardrail? (Decide
   from the competitive research — OpenInference span kinds vs gen_ai operation names.)
4. **Scores/evals.** Is auto/manual score emission in scope, or a separate "evals"
   feature? It overlaps the deferred evals item in IDEAS.md.
5. **Attribute namespacing.** User-supplied keys: pass through verbatim (powerful, but
   risks polluting `gen_ai.*`), or namespace under `sema.user.*`? (Recommend:
   pass-through, with the prototype-pollution key guard already in `otel/event`.)
6. **Capture-content interaction.** Should `otel/llm-span` content respect the same
   `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT` flag? (Recommend: yes — one rule.)
7. **wasm.** All of this must compile to no-ops on wasm (the facade already does).

---

## 4. Effort sketch (rough, if approved)

- **S** (½ day): `otel/set-attribute(s)`, `otel/set-status`, attrs-map on `otel/span`,
  `with-span` macro. (Pure facade plumbing — `SpanCore::set_*` already exist.)
- **M** (1 day): `otel/with-session`/`with-user` (expose `set_conversation_scope`),
  typed helpers `otel/llm-span` + `otel/llm-usage` + `otel/tool-span` +
  `otel/retrieval-span`.
- **L** (depends): `otel/score` / evals — needs the export-path decision (span attr vs
  dedicated sink) and overlaps the evals feature.

---

## 5. Related trace-structure finding (separate from this API)

Observed in a live multi-turn run: under `invoke_agent`, the per-round `chat` spans and
the `execute_tool` spans are **flat siblings** (GenAI-semconv style). OpenInference
nests the tool under the LLM span that requested it. For very long runs the flat list
is still readable (temporal order preserved) but a nested view reads better. This is an
**auto-instrumentation** decision (where `tool_span` attaches in `run_tool_loop`), not
part of the Sema-native API — track separately; pending the competitive research's
verdict on nesting conventions.
