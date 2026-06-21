---
name: "otel/span"
module: "otel"
section: "Observability"
---

Run a thunk inside a named OpenTelemetry INTERNAL span and return the thunk's value. The span ends (recording its duration) when the thunk returns. Any LLM/tool spans created during the thunk nest beneath it. A no-op when telemetry is disabled.

**Signature:** `(otel/span name thunk) → any`

```sema
(otel/span "ingest-batch"
  (fn ()
    (otel/event "started" {:batch-size 100})
    (+ 40 2)))
; => 42  (and emits one INTERNAL span named "ingest-batch")
```

Telemetry is opt-in: spans are only exported when an OTLP endpoint or `SEMA_OTEL_FILE` is configured. See the [Observability guide](/docs/llm/observability).
