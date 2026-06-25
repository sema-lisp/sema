---
name: "otel/span"
module: "otel"
section: "Observability"
---

Run a thunk inside a named OpenTelemetry INTERNAL span and return the thunk's value. The span ends (recording its duration) when the thunk returns, and is marked with Error status if the thunk throws. An optional attributes map is attached to the span. Any LLM/tool spans created during the thunk nest beneath it. A no-op when telemetry is disabled. The `with-span` macro is the ergonomic form: `(with-span name attrs body…)`.

**Signature:** `(otel/span name thunk) → any` · `(otel/span name thunk attrs) → any`

```sema
(otel/span "ingest-batch"
  (fn ()
    (otel/event "started" {:batch-size 100})
    (+ 40 2))
  {:batch.size 100})
; => 42  (and emits one INTERNAL span named "ingest-batch")
```

Telemetry is opt-in: spans are only exported when an OTLP endpoint or `SEMA_OTEL_FILE` is configured. See the [Observability guide](https://sema-lang.com/docs/llm/observability).
