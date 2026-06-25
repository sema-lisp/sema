---
name: "otel/set-status"
module: "otel"
section: "Observability"
---

Set the status of the innermost active span. `:ok` marks success; `:error` marks failure and records an `error.type` attribute plus the optional message. A no-op when telemetry is disabled or there is no active span. (Span-wrapping forms like `otel/span` already set Error status automatically when their body throws — use this for finer control.)

**Signature:** `(otel/set-status :ok) → nil` · `(otel/set-status :error msg) → nil`

```sema
(with-span "upstream" {}
  (if (ok? (call-api))
      (otel/set-status :ok)
      (otel/set-status :error "upstream timeout")))
```

See the [Observability guide](https://sema-lang.com/docs/llm/observability).
