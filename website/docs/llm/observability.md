---
outline: [2, 3]
---

# Observability (OpenTelemetry)

Sema emits standards-compliant [OpenTelemetry](https://opentelemetry.io/) traces and
metrics for every LLM and agent run, following the
[GenAI semantic conventions](https://github.com/open-telemetry/semantic-conventions/tree/main/docs/gen-ai)
that Datadog, Langfuse, Grafana, Honeycomb, Jaeger, and Phoenix consume natively.

Each non-streaming completion becomes one `chat` CLIENT span carrying the provider,
model, token counts, finish reason, and computed cost. Agent runs nest a full
`invoke_agent → (chat, execute_tool …, chat)` tree. Two GenAI metric histograms
(token usage + operation duration) are exported alongside.

It is **off by default and zero-cost when off** — no provider is installed unless you
set an OTLP endpoint or a file sink, and a down/slow collector can never block, add
latency, or crash your script.

## Quick start (Jaeger in one command)

```bash
docker run --rm -d --name jaeger -p 4318:4318 -p 16686:16686 \
  jaegertracing/all-in-one

OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
  sema -e '(llm/complete "say hi" {:model "gpt-5-mini" :max-tokens 16})'
```

Open <http://localhost:16686>, pick the **sema** service, and you'll see a `chat
gpt-5-mini` span with `gen_ai.provider.name`, request/response model, input/output
tokens, `gen_ai.usage.cost_usd`, and the finish reason.

## Configuration

Telemetry is driven entirely by standard `OTEL_*` environment variables (plus a couple
of `SEMA_OTEL_*` conveniences). If **neither** an OTLP endpoint nor `SEMA_OTEL_FILE` is
set, no provider is installed and spans are a no-op.

| Variable | Effect |
| --- | --- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP endpoint — **presence enables export** (e.g. `http://localhost:4318`) |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `http/protobuf` (default) · `http/json` · `grpc` |
| `OTEL_EXPORTER_OTLP_HEADERS` | OTLP headers, e.g. auth (`Authorization=Bearer …`) |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | Per-export timeout in ms (keep short, e.g. `3000`) |
| `OTEL_SERVICE_NAME` | Resource service name (default `sema`) |
| `SEMA_OTEL_FILE=path` | Write spans as JSONL to a file — collector-independent offline capture |
| `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true` | Capture prompt/response **content** on spans (OFF by default; alias `SEMA_OTEL_CAPTURE_CONTENT`) |
| `OTEL_BSP_MAX_QUEUE_SIZE` / `_MAX_EXPORT_BATCH_SIZE` / `_SCHEDULE_DELAY` | Batch processor tuning |

Both OTLP transports (HTTP and gRPC) are always compiled in — switch with
`OTEL_EXPORTER_OTLP_PROTOCOL`, no rebuild needed.

### Offline file capture

No collector? Write spans straight to disk as one JSON object per line:

```bash
SEMA_OTEL_FILE=/tmp/sema-trace.jsonl \
  sema -e '(llm/complete "ping" {:model "gpt-5-mini" :max-tokens 16})'

cat /tmp/sema-trace.jsonl | jq .
```

The file sink writes synchronously, so it captures spans even for short scripts.

### Langfuse (hosted OTLP)

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://cloud.langfuse.com/api/public/otel"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic $(echo -n "$LANGFUSE_PUBLIC_KEY:$LANGFUSE_SECRET_KEY" | base64)"
sema myagent.sema
```

## What gets traced

| Span | Kind | Name | When |
| --- | --- | --- | --- |
| LLM call | `CLIENT` | `chat {model}` | every non-streaming completion (incl. cache hits) |
| Embeddings | `CLIENT` | `embeddings {model}` | every `llm/embed` |
| Tool call | `INTERNAL` | `execute_tool {name}` | every tool dispatch in an agent loop |
| Agent run | `INTERNAL` | `invoke_agent {name}` | every `agent/run` / tools-enabled completion |
| Notebook run | `INTERNAL` | `notebook.run_all` → `notebook.cell {id}` | a notebook "Run All" (one trace, one child per cell) |
| Retry | `INTERNAL` | `llm.retry_attempt` | each HTTP retry (429/5xx/network), under the LLM span |

Key span attributes: `gen_ai.operation.name`, `gen_ai.provider.name`,
`gen_ai.request.model` / `gen_ai.response.model`, `gen_ai.usage.input_tokens` /
`output_tokens`, `gen_ai.usage.cache_read.input_tokens` /
`cache_creation.input_tokens`, `gen_ai.response.finish_reasons`,
`gen_ai.usage.cost_usd`, and `gen_ai.cache.hit` on cached responses. Tool spans carry
`gen_ai.tool.name` / `gen_ai.tool.call.id` / `gen_ai.tool.type`.

### Metrics

When exporting over OTLP, two GenAI histograms are recorded:

- `gen_ai.client.token.usage` (unit `{token}`, dimension `gen_ai.token.type` =
  `input`/`output`)
- `gen_ai.client.operation.duration` (unit `s`)

> Cache hits report **zero** usage by design (no provider call was made), so
> token metrics undercount real spend on cache hits.

## Adding your own spans

Two builtins let Sema code emit custom spans and events. They are no-ops when
telemetry is disabled.

```sema
;; Wrap any work in a named INTERNAL span; returns the thunk's value.
(otel/span "ingest-batch"
  (fn ()
    (otel/event "started" {:batch-size 100})
    (process-batch)))
```

- `(otel/span name thunk)` — run `thunk` inside a span named `name`; the span ends
  (recording its duration) when the thunk returns. LLM/tool spans created inside nest
  beneath it.
- `(otel/event name attrs-map)` — add an event to the current span.

## Privacy

Prompt and response **content** is never captured unless you explicitly set
`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true`. Token counts, models, cost,
and latency are always safe to export and contain no message text. Captured content is
truncated to bound span size.

## Embedding Sema in your own app

When Sema runs as an embedded library, it never installs a global tracer provider on
its own — it emits against whatever provider your application already configured via
`opentelemetry::global`, and its spans automatically nest under your current span
(seeded from `opentelemetry::Context::current()`). If your host has no provider
installed, Sema's spans are a silent no-op. See the embedding guide for details.
