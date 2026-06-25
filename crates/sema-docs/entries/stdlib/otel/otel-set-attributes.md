---
name: "otel/set-attributes"
module: "otel"
section: "Observability"
---

Set many attributes on the innermost active span from a map. Keys are keywords or strings; values keep their type (number/bool/string). Equivalent to calling `otel/set-attribute` for each entry. A no-op when telemetry is disabled or there is no active span.

**Signature:** `(otel/set-attributes attrs-map) → nil`

```sema
(with-span "query" {}
  (otel/set-attributes {:rows 42 :cache.hit true :table "users"}))
```

See the [Observability guide](https://sema-lang.com/docs/llm/observability).
