---
name: "with-span"
module: "otel"
section: "Observability"
---

Macro: run the body inside a named OpenTelemetry span carrying an attributes map, and return the body's value. The span ends on exit (with Error status if the body throws). The ergonomic form of `(otel/span name thunk attrs)`. Use `{}` for no attributes. A no-op when telemetry is disabled.

```sema
(with-span "ingest-batch" {:batch.size 100}
  (otel/set-attribute :rows 42)
  (process-batch))
;; emits one INTERNAL span "ingest-batch"; nested llm/tool spans sit beneath it
```

See the [Observability guide](https://sema-lang.com/docs/llm/observability).
