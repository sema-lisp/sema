---
name: "otel/configure"
module: "otel"
section: "Observability"
---

Point Sema at a tracing backend **from code**, so environment variables aren't the only way to turn telemetry on. Installs an OpenTelemetry provider on the first call and returns `#t` when this call turned tracing on, or `#f` when nothing was configured or telemetry was already active (from the environment at startup, or an earlier `otel/configure`). Only the first call installs a provider — one per process. Call it once, early, before any `llm/*`/`agent/*` work. A no-op on wasm.

Config keys (all optional): `:endpoint` (OTLP url — setting it turns tracing on) · `:file` (write JSONL spans to a path instead of the network — also turns tracing on) · `:protocol` (`"http/protobuf"` default · `"http/json"` · `"grpc"`) · `:key` (an API key, sent as `Authorization: Bearer <key>`) · `:headers` (a map of extra HTTP headers, or a pre-formatted `"name=value,..."` string) · `:service-name` · `:environment` · `:release` · `:capture-content` (bool — record prompt/response text, off by default).

**Signature:** `(otel/configure config-map) → bool`

```sema
;; Hosted backend with an API key (key → Authorization: Bearer …)
(otel/configure {:endpoint "https://cloud.langfuse.com/api/public/otel"
                 :key "sk_prod_123"
                 :service-name "my-agent"})

;; Extra headers as a map
(otel/configure {:endpoint "https://otlp.example.com"
                 :headers {:x-project "checkout" :x-tenant "acme"}})

;; No backend — capture spans to a local JSONL file
(otel/configure {:file "/tmp/sema-trace.jsonl"})
```

Each key maps to the environment variable of the same role (`:endpoint` → `OTEL_EXPORTER_OTLP_ENDPOINT`, `:file` → `SEMA_OTEL_FILE`, `:key`/`:headers` → `OTEL_EXPORTER_OTLP_HEADERS`, …), so a script that configures itself and one driven by env vars behave identically.

See the [Observability guide](https://sema-lang.com/docs/llm/observability).
