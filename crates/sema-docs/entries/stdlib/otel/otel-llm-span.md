---
name: "otel/llm-span"
module: "otel"
section: "Observability"
---

Run a thunk inside a typed **LLM/generation** span for an LLM call you make yourself (a provider Sema doesn't natively support). The config map supplies `:model`, `:provider`, and `:operation` (default `"chat"`); any other keys become span attributes. Sets the `gen_ai.*` request attributes and, when `SEMA_OTEL_COMPAT` is set, the backend-native span-kind — so the call renders as a first-class generation in Phoenix/Traceloop/Langfuse. Account tokens with `otel/llm-usage` inside the thunk. A no-op when telemetry is disabled.

**Signature:** `(otel/llm-span config-map thunk) → any`

```sema
(otel/llm-span {:model "custom-model" :provider "myco" :operation "chat"}
  (fn ()
    (let ((resp (my-http-llm-call prompt)))
      (otel/llm-usage {:input-tokens 120 :output-tokens 30 :cost-usd 0.001})
      resp)))
```

See the [Observability guide](https://sema-lang.com/docs/llm/observability).
