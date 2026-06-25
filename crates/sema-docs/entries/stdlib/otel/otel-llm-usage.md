---
name: "otel/llm-usage"
module: "otel"
section: "Observability"
---

Record LLM token usage and cost on the innermost active span (typically inside an `otel/llm-span`). Emits the same `gen_ai.usage.*` keys as the built-in `llm/*` path plus the active backend's compat aliases, so a custom-provider call accounts identically. Map keys: `:input-tokens`, `:output-tokens`, `:cost-usd`. A no-op when telemetry is disabled or there is no active span.

**Signature:** `(otel/llm-usage usage-map) → nil`

```sema
(otel/llm-span {:model "custom-model" :provider "myco"}
  (fn ()
    (otel/llm-usage {:input-tokens 120 :output-tokens 30 :cost-usd 0.001})
    "done"))
```

See the [Observability guide](https://sema-lang.com/docs/llm/observability).
