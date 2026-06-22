---
outline: [2, 3]
---

# Backend Compatibility

Sema emits the **canonical OpenTelemetry GenAI semantic conventions** (`gen_ai.*`) by
default. That makes its traces render first-class — with **zero configuration** — in any
standards-conforming backend. But a few popular LLM-observability tools key off their
own attribute namespaces instead of `gen_ai.*`. For those, set `SEMA_OTEL_COMPAT` and
Sema *also* emits each tool's native alias keys, so it renders first-class there too —
still with **no manual instrumentation**.

The canonical `gen_ai.*` attributes stay the source of truth; compat only *adds* alias
keys. See the [Observability guide](./observability) for the base feature.

## Supported tools

| Tool | Works with **zero config**? | `SEMA_OTEL_COMPAT` token | What compat adds |
| --- | --- | --- | --- |
| Vanilla OTel (Grafana/Tempo, Jaeger) | ✅ yes | — | — |
| **Logfire** (Pydantic) | ✅ yes | — | — |
| **Datadog** LLM Observability | ✅ yes | — | — |
| **Honeycomb** | ✅ yes | — | — |
| **SigNoz** | ✅ yes | — | — |
| **Elastic** / **New Relic** | ✅ yes | — | — |
| **OpenLIT** | ✅ yes | — | — |
| **Braintrust** | ✅ yes (reads `gen_ai.*`) | `braintrust` | tags + metadata + cost metric |
| **Langfuse** | ⚠️ partial | `langfuse` | observation type/model/usage/cost, trace I/O rollup, tags/metadata |
| **Arize Phoenix** / Arize AX (OpenInference) | ❌ needs compat | `openinference` (`phoenix`, `arize`) | span kinds, model/provider, tokens, cost, I/O, tool args/schemas |
| **Traceloop** (OpenLLMetry) | ⚠️ partial | `traceloop` (`openllmetry`) | span kinds, entity I/O, indexed usage, tool functions |
| **LangSmith** (LangChain) | ⚠️ partial | `langsmith` | run-type kinds, session id, tags/metadata |
| **Helicone** | ❌ not via OTel push | — | use its gateway/proxy — not an attribute gap |

> The 8 "zero config" tools consume the standard `gen_ai.*` semconv directly — Sema is
> already best-in-class there with nothing to set. Compat exists for the tools that
> don't.

## Configuration

| Variable | Effect |
| --- | --- |
| `SEMA_OTEL_COMPAT` | Comma-separated tokens (case-insensitive). Default unset = `gen_ai.*` only. |
| `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true` | Required for the I/O / tool-args / trace-rollup aliases (content is off by default). |
| `OTEL_EXPORTER_OTLP_ENDPOINT` / `_PROTOCOL` | Where to send (see the Observability guide). |
| `SEMA_OTEL_ENVIRONMENT` | Becomes `deployment.environment.name` (filterable in Langfuse/Logfire). |

Tokens: `openinference` (aliases `phoenix`, `arize`), `traceloop` (alias `openllmetry`),
`langsmith`, `langfuse`, `braintrust`, and `all`. Unknown tokens are ignored.

```bash
# Render Sema traces natively in Phoenix:
SEMA_OTEL_COMPAT=openinference \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:6006 \
OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true \
  sema myagent.sema

# Belt-and-suspenders: emit every backend's keys at once.
SEMA_OTEL_COMPAT=all  sema myagent.sema
```

When `SEMA_OTEL_COMPAT` is unset, **no alias attributes are emitted** — spans are
byte-identical to the canonical `gen_ai.*` output, with zero added cost.

## Per-tool setup

### Arize Phoenix (OpenInference)

Phoenix is the dominant self-hostable LLM-trace UI. One container:

```bash
docker run -d --name phoenix -p 6006:6006 -p 4317:4317 arizephoenix/phoenix:latest
# UI: http://localhost:6006 — OTLP HTTP on :6006 (/v1/traces), gRPC on :4317

SEMA_OTEL_COMPAT=openinference \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:6006 \
OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true \
  sema -e '(llm/complete "say hi" {:max-tokens 16})'
```

You'll see each Sema span classified (`LLM` / `TOOL` / `AGENT` / `EMBEDDING`) with model,
provider, token counts, cost, the message I/O, and — for agents — tool arguments,
results, and the advertised tool schemas.

### Langfuse

Self-hosted Langfuse already reads Sema's `gen_ai.usage.cost` and
`langfuse.observation.input/output`. Adding `langfuse` fills the rest — observation
type/model, usage/cost detail objects, and the **trace-level** input/output panel:

```bash
SEMA_OTEL_COMPAT=langfuse \
OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:3000/api/public/otel" \
OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic <base64(pk:sk)>" \
OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true \
  sema myagent.sema
```

Multi-turn runs group into [Sessions](./observability#sessions-users-multi-turn-grouping)
via the `:session-id` / `:user-id` options.

### Traceloop (OpenLLMetry)

Traceloop is mostly SaaS but emits plain OTLP — verify with any OSS OTLP backend (e.g.
SigNoz). `SEMA_OTEL_COMPAT=traceloop` adds `traceloop.span.kind`, `traceloop.entity.*`,
the indexed token keys, and `llm.request.functions.*`.

### LangSmith

LangSmith ingestion is cloud-only (no local image). Point Sema at its OTLP endpoint with
your API key and `SEMA_OTEL_COMPAT=langsmith` to get LangSmith run types, session
threading (`langsmith.trace.session_id`), and tags/metadata.

### Braintrust

Braintrust already maps `gen_ai.*` natively, so it works with no token. Add `braintrust`
only if you want native `braintrust.tags` / `braintrust.metadata`.

## Span-kind mapping reference

| Sema span | OpenInference | Traceloop | LangSmith | Langfuse |
| --- | --- | --- | --- | --- |
| `chat` | `LLM` | `task` | `llm` | `generation` |
| `embeddings` | `EMBEDDING` | `task` | `embedding` | `generation` |
| `execute_tool` | `TOOL` | `tool` | `tool` | `span` |
| `invoke_agent` | `AGENT` | `agent` | `chain` | `span` |
| `vm` / cell / retry | `CHAIN` | `workflow` | `chain` | `span` |

## Limitations

- **Content-bearing aliases require the opt-in flag.** Message I/O, tool arguments/
  results, and the trace-level rollup only appear with
  `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true`. Token counts, models, cost,
  and span kinds are always emitted.
- **OpenInference has no tool-result key** — the result is surfaced via `output.value`
  on the tool span (not a dedicated attribute).
- **LangSmith recomputes cost** server-side from tokens; Sema's exact per-call cost
  (including cache pricing) won't be the number it shows unless its pricing map matches.
- **Helicone** is not an OTLP `/v1/traces` receiver — it's a gateway/proxy. Compat can't
  bridge it; use its gateway integration instead.
- **Not yet implemented (roadmap):** streaming time-to-first-token
  (`langfuse.observation.completion_start_time`) and the per-message *indexed* form
  (`gen_ai.prompt.{i}.*`) used by some legacy Traceloop/LangSmith parsers — the
  structured-JSON and `entity.*` forms are emitted today. Auto-tags and a Sema-level
  `:tags` / `:metadata` option are also planned.
- **Attribute bloat:** compat adds extra attributes per span. Leave `SEMA_OTEL_COMPAT`
  unset for pure vanilla-OTel pipelines that don't need the aliases.
