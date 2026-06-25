---
name: "otel/event"
module: "otel"
section: "Observability"
---

Add an event (with optional attributes) to the current OpenTelemetry span. Attribute values are stringified. Returns nil. A no-op when telemetry is disabled or when there is no active span.

**Signature:** `(otel/event name) → nil` · `(otel/event name attrs-map) → nil`

```sema
(otel/span "request"
  (fn ()
    (otel/event "cache-miss" {:key "user:42"})
    (fetch-user 42)))
```

Telemetry is opt-in: events are only exported when an OTLP endpoint or `SEMA_OTEL_FILE` is configured. See the [Observability guide](https://sema-lang.com/docs/llm/observability).
