---
name: "otel/set-attribute"
module: "otel"
section: "Observability"
---

Set one attribute on the innermost active span (the current `otel/span`, `agent/run`, or `llm/*` span). The key is a keyword or string; the value keeps its type — integers, floats, and booleans render as numbers/bools in the backend, not strings. A no-op when telemetry is disabled or there is no active span.

**Signature:** `(otel/set-attribute key value) → nil`

```sema
(with-span "fetch" {}
  (otel/set-attribute :http.status 200)
  (otel/set-attribute :cache.hit true))
```

See the [Observability guide](https://sema-lang.com/docs/llm/observability).
