---
outline: [2, 3]
---

# Backend Compatibility

By default Sema labels its telemetry with the
[OpenTelemetry GenAI semantic conventions](https://github.com/open-telemetry/semantic-conventions/tree/main/docs/gen-ai)
— the standard `gen_ai.*` attribute names. Tools that follow that standard understand
Sema's traces with no extra configuration.

A handful of popular LLM-observability tools don't read `gen_ai.*` — they look for their
own attribute names instead, so a Sema span can show up in them as "unknown" or with
blank fields. For those tools, set the `SEMA_OTEL_COMPAT` environment variable to a
**compatibility mode** — a short name such as `openinference` or `langfuse` that tells
Sema which extra attribute names to write *alongside* the standard ones. Nothing about
your program changes — it's still the same automatic tracing, just labelled so more tools
can read it.

This is purely additive: the standard `gen_ai.*` attributes are always present;
`SEMA_OTEL_COMPAT` only adds extra copies under other names. Read the
[Tracing & Metrics](./observability) page first for how tracing works and how to point
Sema at a backend — this page only covers the per-tool labelling.

## Which tools need a compatibility mode

This section covers the tools that can **receive** OpenTelemetry traces over OTLP. Most of
them read the standard `gen_ai.*` attributes and need **no** compatibility mode; only a few
key off their own attribute names and need a `SEMA_OTEL_COMPAT` mode. (Tools that ingest
only through their own SDK, sit in front of your calls as a proxy, or run offline
evaluations can't receive an OTLP push at all — see
[Tools you can't send traces to](#tools-you-can-t-send-traces-to).)

::: tip "No compatibility mode" is not the same as "no setup."
Almost every **hosted** service still needs its own authentication header — an API key
passed through `OTEL_EXPORTER_OTLP_HEADERS`, exactly as shown for Langfuse on the
[Tracing & Metrics](./observability#sending-to-hosted-langfuse) page. That auth header is a
property of the *backend*, not a Sema compatibility mode. The tables below say only whether
a tool needs a `SEMA_OTEL_COMPAT` mode to *understand* Sema's attributes — the column for
"does it need an API key" is "almost always, if it's hosted".
:::

### Reads `gen_ai.*` — no compatibility mode needed

**General trace viewers and APM platforms.** These store and display `gen_ai.*` as ordinary
span attributes (the LLM-specific ones also build GenAI dashboards from them):

| Tool | Self-hostable? | Notes |
| --- | --- | --- |
| Grafana / Tempo, [Jaeger](https://www.jaegertracing.io/) | yes | plain OpenTelemetry trace viewers |
| [SigNoz](https://signoz.io/) | yes | OTLP on 4317 / 4318 |
| [OpenObserve](https://openobserve.ai/) | yes | OTLP `/api/{org}/v1/traces` *(verified live)* |
| Honeycomb, Elastic | partly | general OTel APM |
| [Logfire](https://pydantic.dev/logfire) | no | Pydantic's OTel platform |
| [Datadog](https://www.datadoghq.com/) LLM Observability | no | maps `gen_ai.*` (semconv 1.37+) natively; needs a Datadog API-key header |
| [Dynatrace](https://www.dynatrace.com/) | no | maps `gen_ai.*` natively; needs a Grail (DPS) licence + an ingest token |
| [Coralogix](https://coralogix.com/) AI Center | no | maps `gen_ai.*`, but needs account-side setup (S3-archive routing + the experimental-semconv opt-in) |
| [New Relic](https://newrelic.com/) | no | accepts OTLP and stores `gen_ai.*` as raw attributes; native GenAI dashboards are not documented |

**LLM-native platforms.** These parse `gen_ai.*` into structured LLM records on their own
OTLP endpoint (all hosted ones need an API key/header):

| Tool | Self-hostable? | OTLP endpoint / notes |
| --- | --- | --- |
| [OpenLIT](https://openlit.io/) | yes | OTel-native; `docker compose up -d`; OTLP on 4318, no auth by default |
| [MLflow](https://mlflow.org/) | yes | tracking server exposes an OTLP `/v1/traces` endpoint |
| [Braintrust](https://www.braintrust.dev/) | no | maps `gen_ai.*` to structured fields; API key required (see the optional `braintrust` mode below) |
| [W&B Weave](https://wandb.ai/) | no | `…/otel/v1/traces`; parses `gen_ai.*`; Basic-auth + `project_id` header *(verified in docs)* |
| [Portkey](https://portkey.ai/) | no | `/v1/otel/v1/traces`; reads `gen_ai.*`; `x-portkey-api-key` header |
| [HoneyHive](https://honeyhive.ai/) | no | `/v1/traces`; reads `gen_ai.*`; Bearer + `x-honeyhive` project header |
| [Opik](https://www.comet.com/opik) (Comet) | yes | `/api/v1/private/otel` (HTTP only); API key + project/workspace headers |
| [Lunary](https://lunary.ai/) | yes | `/v1/otel`; reads `gen_ai.*`; Bearer token |
| [Maxim AI](https://www.getmaxim.ai/) | no | `/v1/otel`; reads `gen_ai.*` / `llm.*` / `ai.*`; `x-maxim-*` headers |
| [PostHog](https://posthog.com/) | yes | `/i/v0/ai/otel`; maps `gen_ai.*` → `$ai_*` events; project token |
| [FutureAGI](https://futureagi.com/) | no | native convention is `gen_ai.*` (+ `fi.span.kind`); OpenInference is only an optional output mode |
| [Laminar](https://www.lmnr.ai/) | yes | parses `gen_ai.*` (+ its own `lmnr.*`); HTTP + gRPC; API key |
| [Agenta](https://agenta.ai/) | yes | translates `gen_ai.*` into its own `ag.*`; HTTP/protobuf only; API key |
| [Confident AI](https://www.confident-ai.com/) | no | Observatory endpoint reads `gen_ai.*` (+ `confident.*`); API key — this is DeepEval's backend |
| [Patronus AI](https://docs.patronus.ai/) | no | OTLP gRPC; ingests standard OTel spans; `x-api-key` header |
| [Promptfoo](https://www.promptfoo.dev/) | local | built-in OTLP receiver (port 4318) **while `promptfoo eval` runs**; no token |

### Needs a compatibility mode

These ingest OTLP but key off their **own** attribute names, so without the matching
`SEMA_OTEL_COMPAT` mode a Sema span shows up with blank or "unknown" fields:

| Tool | `SEMA_OTEL_COMPAT` mode | What it adds |
| --- | --- | --- |
| [Arize Phoenix](https://phoenix.arize.com/), [Arize AX](https://arize.com/) | `openinference` | span types, model/provider, tokens, cost, message I/O, tool args + schemas |
| [Langfuse](https://langfuse.com/) | `langfuse` | observation type/model, usage + cost detail, trace-level input/output, tags |
| [Traceloop](https://www.traceloop.com/) / OpenLLMetry | `traceloop` | span types, entity input/output, indexed message keys, tool functions |
| [LangSmith](https://www.langchain.com/langsmith) | `langsmith` | run types, session threading, tags/metadata |
| [Braintrust](https://www.braintrust.dev/) | `braintrust` *(optional)* | adds the richer `braintrust.*` tags/metadata/scores (it already reads `gen_ai.*` without it) |

> **Often grouped with OpenLLMetry, but actually `gen_ai.*`-native:** Laminar, LangWatch,
> Agenta and FutureAGI are sometimes listed as "Traceloop-compatible". In practice they read
> `gen_ai.*` directly (Agenta and FutureAGI translate it into their own namespace), so they
> need **no** compatibility mode — they're in the table above. The OpenLLMetry SDK works with
> them because *it too* emits `gen_ai.*`, not because they parse the `traceloop.*` namespace.

> **Advertise OTel but unconfirmed:** Galileo, PromptLayer, Keywords AI, Arthur AI, and
> [LangWatch](https://langwatch.ai/) accept OTLP or claim OpenTelemetry support, but their
> docs don't pin down which attributes they surface from a raw push. They may well work —
> send a trace with the standard setup and check whether your spans appear.

## Setting `SEMA_OTEL_COMPAT`

It's an environment variable like the others (see
[How to turn it on](./observability#how-to-turn-it-on)). Its value is a comma-separated
list of compatibility modes — the lower-case names from the table above:

```bash
# Just Phoenix:
SEMA_OTEL_COMPAT=openinference sema myagent.sema

# Phoenix and Langfuse at once:
SEMA_OTEL_COMPAT=openinference,langfuse sema myagent.sema

# Every mode at once — useful if you're not sure which backend you'll use:
SEMA_OTEL_COMPAT=all sema myagent.sema
```

Accepted modes: `openinference` (also `phoenix`, `arize`), `traceloop` (also
`openllmetry`), `langsmith`, `langfuse`, `braintrust`, and `all`. Names you don't
recognise are ignored, so a typo won't break anything.

Some of the added detail — message text, tool arguments and results, and the trace-level
input/output summary — is **content**, so it only appears when you also turn on content
capture with `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true` (see
[Privacy](./observability#privacy)). Token counts, models, cost, and span types are
always added.

When `SEMA_OTEL_COMPAT` is unset, no extra attributes are written — the traces are exactly
what you get on the [Tracing & Metrics](./observability) page.

## Per-tool setup

### Arize Phoenix (OpenInference)

Phoenix is an open-source LLM trace viewer that runs in one container:

```bash
# Start Phoenix. UI on 6006; it accepts traces on 6006 (HTTP) and 4317 (gRPC).
docker run -d --name phoenix -p 6006:6006 -p 4317:4317 arizephoenix/phoenix:latest

SEMA_OTEL_COMPAT=openinference \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:6006 \
OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true \
  sema -e '(llm/complete "say hi" {:max-tokens 16})'
```

Open `http://localhost:6006`. Each Sema span is typed (`LLM` / `TOOL` / `AGENT` /
`EMBEDDING`) and shows the model, provider, token counts, cost, the message I/O, and —
for agent runs — tool arguments, results, and the tool schemas offered to the model.

### Langfuse

Langfuse already reads several of Sema's standard attributes (cost and message I/O). The
`langfuse` value fills in the rest — the observation type and model, the usage/cost detail
objects, and the trace-level input/output summary:

```bash
SEMA_OTEL_COMPAT=langfuse \
OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:3000/api/public/otel" \
OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic <base64 of publickey:secretkey>" \
OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true \
  sema myagent.sema
```

(See the [Langfuse example](./observability#sending-to-hosted-langfuse) for how to build
the auth header.) Multi-turn runs group into
[Sessions](./observability#sessions-and-users-grouping-multi-turn-runs) via the
`:session-id` and `:user-id` options.

### Traceloop (OpenLLMetry)

Traceloop is mainly a hosted product, but it reads plain OTLP, so you can also view the
output in any OTLP backend (such as SigNoz). `SEMA_OTEL_COMPAT=traceloop` adds the
`traceloop.span.kind` and `traceloop.entity.*` attributes, the indexed per-message keys,
and the advertised tool functions. (You only need this for Traceloop's own platform — the
look-alikes Laminar, LangWatch and Agenta read `gen_ai.*` directly.)

### LangSmith

Point Sema at LangSmith's OTLP endpoint with your API key and `SEMA_OTEL_COMPAT=langsmith`;
this adds LangSmith's run types, session threading, and tags/metadata, which are needed for
those features (`gen_ai.*` alone can't populate them). LangSmith is primarily hosted, but
Enterprise self-hosted deployments expose their own OTLP endpoint too.

### Braintrust

Braintrust reads the standard attributes, so it works with no value set. Add `braintrust`
only if you want its native `braintrust.tags` and `braintrust.metadata` fields.

## Span-type mapping

How each Sema span is labelled for each tool when its compat value is on:

| Sema span | OpenInference | Traceloop | LangSmith | Langfuse |
| --- | --- | --- | --- | --- |
| `chat` | `LLM` | `task` | `llm` | `generation` |
| `embeddings` | `EMBEDDING` | `task` | `embedding` | `generation` |
| `execute_tool` | `TOOL` | `tool` | `tool` | `span` |
| `invoke_agent` | `AGENT` | `agent` | `chain` | `span` |
| notebook cell / retry | `CHAIN` | `workflow` | `chain` | `span` |

## Tools you can't send traces to

Some LLM tools collect data a different way — through their own client SDK, by sitting in
front of your API calls as a proxy, or by running offline evaluations — rather than by
receiving OpenTelemetry traces. Sema's OTLP export can't feed those; to use one, follow
its own integration guide instead. The main categories:

- **Proxies / gateways** — capture by routing your model calls through them, not by
  accepting traces: [Helicone](https://www.helicone.ai/), [LiteLLM](https://litellm.ai/),
  [Pezzo](https://pezzo.ai/). (Portkey is *not* here — its observability endpoint accepts
  OTLP and reads `gen_ai.*`; see the table above.)
- **SDK-only platforms** — ingest only through their own Python/JS library, with no OTLP
  trace endpoint: [Vellum](https://www.vellum.ai/), [Athina AI](https://athina.ai/),
  [Parea AI](https://www.parea.ai/), [Nebuly](https://www.nebuly.com/).
- **Evaluation-only** — offline scoring/testing, not a runtime trace receiver:
  [RAGAS](https://docs.ragas.io/), [UpTrain](https://uptrain.ai/),
  [Evidently AI](https://www.evidentlyai.com/), [Giskard](https://www.giskard.ai/),
  [TruLens](https://www.trulens.org/).
- **Guardrails libraries** that *emit* telemetry rather than receive it:
  [NVIDIA NeMo Guardrails](https://github.com/NVIDIA/NeMo-Guardrails),
  [Guardrails AI](https://www.guardrailsai.com/).
- **Has an OTLP endpoint, but needs attributes Sema doesn't emit** —
  [Fiddler AI](https://www.fiddler.ai/) accepts OTLP/HTTP, but requires its own
  `fiddler.span.type` and `application.id` on every span; without them spans are dropped,
  and Sema has no Fiddler compatibility mode to add them.

> Several tools that *used* to be SDK-only or eval-only now run an OTLP endpoint — Opik,
> Lunary, PostHog, Maxim, Promptfoo, Patronus and Confident AI are all in the supported
> tables above. [Humanloop](https://humanloop.com/) is gone the other way: its team joined
> Anthropic and the platform was sunset in September 2025, so it's no longer an integration
> target. If a tool below later adds an OTLP endpoint that reads the GenAI conventions, Sema
> works with it the same as the others — no change needed on Sema's side.

## Limitations

- **Message content requires the opt-in flag.** The message I/O, tool arguments and
  results, and the trace-level input/output only appear when
  `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true`. Token counts, models, cost,
  and span types are always added.
- **OpenInference has no separate tool-result field** — the result appears in the tool
  span's `output.value` rather than a dedicated attribute.
- **A backend may re-derive cost** from the token counts on its side rather than reading
  Sema's `gen_ai.usage.cost`, so the figure it shows can differ from Sema's exact per-call
  cost (which accounts for cache pricing).
- **Proxies and gateways can't receive traces.** Helicone, LiteLLM and Pezzo capture data
  by routing your model calls through them, not by accepting an OTLP push — use their own
  integration instead.
- **Not yet implemented:** streaming time-to-first-token, and the per-message *indexed*
  attribute form some older Traceloop/LangSmith parsers expect (Sema emits the structured
  and entity forms today). An auto-tagging option is also planned.
- **More attributes per span.** Compat adds extra copies of each value. If you only use a
  plain OTel backend, leave `SEMA_OTEL_COMPAT` unset to keep spans lean.
