---
outline: [2, 3]
---

# Tracing & Metrics

Sema can record what happens inside every LLM and agent run — each model call, tool
execution, retry, and notebook cell — as [OpenTelemetry](https://opentelemetry.io/)
traces and metrics, and send them to a tool where you can browse them. You don't write
any instrumentation: switch it on with one environment variable — or one
[`otel/configure`](#configuring-from-sema-code) call — and `llm/complete`, `agent/run`,
`llm/embed`, and the rest are recorded automatically.

If OpenTelemetry is new to you, the terms used below:

- **OpenTelemetry (OTel)** is an open, vendor-neutral standard for traces and metrics.
- A **trace** is one run. It is made of **spans** — individual timed operations such as a
  single LLM call or a tool execution. Spans nest, so an agent run appears as a tree.
- **OTLP** is the network protocol OTel uses. Sema speaks OTLP, so it works with any tool
  that accepts it — a free local viewer like [Jaeger](https://www.jaegertracing.io/), or
  a hosted service like [Langfuse](https://langfuse.com/), Grafana, or Datadog.
- Sema follows the OTel
  [GenAI semantic conventions](https://github.com/open-telemetry/semantic-conventions-genai)
  — the agreed attribute names for LLM telemetry (token counts, model, cost, …) — so
  these tools understand the data with no per-tool glue. Grafana, Jaeger, SigNoz,
  OpenObserve, Datadog, Honeycomb, Logfire, MLflow, and others read it as-is; a few
  LLM-specific tools (Arize Phoenix, Langfuse, …) need one extra setting — see
  [Backend Compatibility](./otel-compat).

Tracing is **off by default** — if you don't point Sema at a backend or a file, it
records nothing. And once it's on, a slow or unreachable backend can never block, delay,
or crash your script: telemetry is sent in the background, out of the way of your run.

## How to turn it on

The usual way is **environment variables** — values you set in your shell. (To turn
tracing on from inside a script instead, see
[Configuring from Sema code](#configuring-from-sema-code) below.) You can set them inline
for a single command, or `export` them for the whole session:

```bash
# Inline — applies to this one run only:
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 sema myscript.sema

# Or exported — applies to every command in this shell session:
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
sema myscript.sema
```

Two variables decide *where* the data goes, and **setting either one turns tracing on**:

- `OTEL_EXPORTER_OTLP_ENDPOINT` — send to a backend over the network (Jaeger, Langfuse, …).
- `SEMA_OTEL_FILE` — write to a local file instead (handy with no backend at all).

Set neither and tracing stays off. The full list of variables is in
[Configuration reference](#configuration-reference) below.

### Configuring from Sema code

Environment variables aren't the only way. `otel/configure` turns tracing on from inside
a script, so a program can point itself at a backend without any shell setup:

```sema
;; Hosted backend with an API key — :key becomes an Authorization: Bearer header.
(otel/configure {:endpoint "https://cloud.langfuse.com/api/public/otel"
                 :key "sk_prod_123"
                 :service-name "my-agent"})

(llm/complete "say hi" {:max-tokens 16})   ; recorded against that backend
```

Call it once, early — before any `llm/*` or `agent/*` work. It installs a provider on
the first call and returns `#t` when this call turned tracing on, or `#f` when nothing was
configured or telemetry was already active (from the environment at startup, or an earlier
`otel/configure`). One provider is installed per process, so environment configuration —
if present — wins, and a later `otel/configure` is a no-op.

The config map accepts:

| Key | Maps to | What it does |
| --- | --- | --- |
| `:endpoint` | `OTEL_EXPORTER_OTLP_ENDPOINT` | Backend address. **Setting it turns tracing on.** |
| `:file` | `SEMA_OTEL_FILE` | Write JSONL spans to a path instead of the network. Also turns tracing on. |
| `:protocol` | `OTEL_EXPORTER_OTLP_PROTOCOL` | `"http/protobuf"` (default) · `"http/json"` · `"grpc"`. |
| `:key` | `OTEL_EXPORTER_OTLP_HEADERS` | An API key, sent as `Authorization: Bearer <key>`. |
| `:headers` | `OTEL_EXPORTER_OTLP_HEADERS` | Extra headers — a map (`{:x-project "app"}`) or a pre-formatted `"name=value,..."` string. |
| `:service-name` | `OTEL_SERVICE_NAME` | The name runs appear under. |
| `:environment` | `SEMA_OTEL_ENVIRONMENT` | Deployment label (`prod`, `staging`, …). |
| `:release` | `SEMA_OTEL_RELEASE` | Release/version stamp. |
| `:capture-content` | `SEMA_OTEL_CAPTURE_CONTENT` | `#t` to record prompt/response text (off by default — see [Privacy](#privacy)). |

Each key maps to the environment variable of the same role, so a script that configures
itself behaves exactly like one driven by the environment. Extra headers as a map:

```sema
(otel/configure {:endpoint "https://otlp.example.com"
                 :headers {:x-project "checkout" :x-tenant "acme"}})
```

Or capture to a local file with no backend at all:

```sema
(otel/configure {:file "/tmp/sema-trace.jsonl"})
```

## Quick start: see a trace in one minute

[Jaeger](https://www.jaegertracing.io/) is a free trace viewer that runs in a single
container — a good way to see your first trace.

```bash
# 1. Start Jaeger. The UI is on port 16686; it accepts traces on 4318.
docker run --rm -d --name jaeger -p 4318:4318 -p 16686:16686 \
  -e COLLECTOR_OTLP_ENABLED=true jaegertracing/all-in-one

# 2. Point Sema at it and run something. No model is pinned here, so this uses
#    your default provider and its default model — just make sure an API key is
#    set (ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY / …).
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
  sema -e '(llm/complete "say hi" {:max-tokens 16})'
```

Open `http://localhost:16686` in your browser, pick the **sema** service, and you'll see
one trace whose `chat` span carries the provider, model, input/output token counts,
cost, and finish reason.

> **Choosing a specific model.** The example uses whichever provider is active. To pick
> one, select it first: `(llm/set-default :openai)` then `{:model "gpt-5-mini"}`, or
> `(llm/set-default :anthropic)` then `{:model "claude-haiku-4-5-20251001"}`. A model id
> only works with the provider that offers it — sending an OpenAI model id to Anthropic
> returns a 404.

## Configuration reference

Every setting is an environment variable (see [How to turn it on](#how-to-turn-it-on)
for how to set them) — or the matching `otel/configure` key (see
[Configuring from Sema code](#configuring-from-sema-code)). The `OTEL_*` names come from
OpenTelemetry itself; the `SEMA_OTEL_*` names are Sema conveniences.

| Variable | What it does |
| --- | --- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | The address of your tracing backend, e.g. `http://localhost:4318`. **Setting this turns tracing on.** |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | How to talk to it: `http/protobuf` (default) · `http/json` · `grpc`. Keep the default unless your backend only accepts gRPC. |
| `OTEL_EXPORTER_OTLP_HEADERS` | Extra HTTP headers, usually authentication — e.g. `Authorization=Bearer <token>`. Comma-separated `name=value` pairs; see [Authentication headers](#authentication-headers). |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | Per-export timeout in milliseconds. Keep it short (e.g. `3000`) so a dead backend never holds things up. |
| `OTEL_SERVICE_NAME` | The name your runs appear under in the backend (default `sema`). |
| `SEMA_OTEL_FILE` | Write traces to this file path, one JSON object per line, instead of sending them over the network. Also turns tracing on. |
| `SEMA_OTEL_ENVIRONMENT` | A label such as `prod` or `staging` for filtering (recorded as `deployment.environment.name`). |
| `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT` | Set to `true` to also record the **prompt and response text** (off by default — see [Privacy](#privacy)). Sema also accepts the shorter alias `SEMA_OTEL_CAPTURE_CONTENT`. |
| `OTEL_BSP_MAX_QUEUE_SIZE`, `OTEL_BSP_MAX_EXPORT_BATCH_SIZE`, `OTEL_BSP_SCHEDULE_DELAY` | Advanced: tune the background export batching. The defaults are fine for most uses. |

Sema can send over HTTP or gRPC; choose with `OTEL_EXPORTER_OTLP_PROTOCOL`. HTTP (the
default) is what most backends accept — only switch to gRPC if yours requires it.

### Writing to a file instead of a backend

No backend running? Set `SEMA_OTEL_FILE` and Sema writes each finished span to a file as
one JSON object per line:

```bash
SEMA_OTEL_FILE=/tmp/sema-trace.jsonl \
  sema -e '(llm/complete "ping" {:max-tokens 16})'

cat /tmp/sema-trace.jsonl | jq .
```

The file is written synchronously, so even a one-line script captures its spans.

## Authentication headers

Almost every **hosted** backend needs an API key, and you pass it as an HTTP header through
`OTEL_EXPORTER_OTLP_HEADERS`. (This is separate from `SEMA_OTEL_COMPAT`, which only relabels
attribute names — see [Backend Compatibility](./otel-compat).) The header **name** and the
key are dictated by the backend, not by Sema; always check the tool's own OTLP page for the
exact names.

### The format

`OTEL_EXPORTER_OTLP_HEADERS` is a comma-separated list of `name=value` pairs — the
[W3C Baggage](https://www.w3.org/TR/baggage/) format the OpenTelemetry spec mandates:

```bash
# one header
OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer sk-abc123"

# two headers — separate with a comma
OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer sk-abc123,x-project=my-app"
```

Rules worth knowing:

- **Separate multiple headers with commas**, not semicolons — semicolons are not supported.
- **The first `=` splits the name from the value**, so the value itself may contain `=`.
  base64 strings with `=` padding (common in Basic auth) work fine.
- **Avoid literal commas or spaces inside a value** — a comma starts a new header. If a token
  genuinely needs one, percent-encode it (`,` → `%2C`). Bearer tokens and base64 never
  contain commas, so this rarely comes up.
- **Quote the whole value in your shell** so `$(...)` substitutions and special characters
  survive.

### Common patterns

| Auth style | `OTEL_EXPORTER_OTLP_HEADERS` value | Example tools |
| --- | --- | --- |
| Bearer token | `Authorization=Bearer <api-key>` | Braintrust, Lunary, LangSmith |
| Basic auth | `Authorization=Basic <base64 of id:secret>` | Langfuse, W&B Weave |
| Vendor key header | `x-portkey-api-key=<key>` · `dd-api-key=<key>` | Portkey, Datadog |

### Building a Basic-auth header

Basic auth wants base64 of `id:secret`. Build it with `base64` and read the keys from
environment variables rather than hard-coding them. For [Langfuse](https://langfuse.com/):

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://cloud.langfuse.com/api/public/otel"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic $(echo -n "$LANGFUSE_PUBLIC_KEY:$LANGFUSE_SECRET_KEY" | base64)"
sema myagent.sema
```

### Tools that need more than one header

Some backends need a second (or third) header to route the trace to the right project or
workspace — the auth key alone isn't enough. The exact names come from each tool's OTLP docs:

| Tool | `OTEL_EXPORTER_OTLP_HEADERS` value |
| --- | --- |
| HoneyHive | `Authorization=Bearer <key>,x-honeyhive=project:<name>` |
| W&B Weave | `Authorization=Basic <base64 of api:KEY>,project_id=<entity>/<project>` |
| Maxim | `x-maxim-api-key=<key>,x-maxim-repo-id=<repo-id>` |
| Opik | `Authorization=<key>,projectName=<project>,Comet-Workspace=<workspace>` |

Several LLM-focused backends can also show **richer** detail with one extra compatibility
setting on top of the auth header — see [Backend Compatibility](./otel-compat).

## What gets traced

| Span | Kind | Name | When |
| --- | --- | --- | --- |
| LLM call | `CLIENT` | `chat {model}` | every non-streaming completion (including cache hits) |
| Embeddings | `CLIENT` | `embeddings {model}` | every `llm/embed` |
| Tool call | `INTERNAL` | `execute_tool {name}` | every tool dispatch in an agent loop |
| Agent run | `INTERNAL` | `invoke_agent {name}` | every `agent/run` / tools-enabled completion |
| Notebook run | `INTERNAL` | `notebook.run_all` → `notebook.cell {id}` | a notebook "Run All" (one trace, one child span per cell) |
| Retry | `INTERNAL` | `llm.retry_attempt` | each HTTP retry (429 / 5xx / network), nested under the LLM span |

Each LLM span carries the standard GenAI attributes: `gen_ai.operation.name`,
`gen_ai.provider.name`, `gen_ai.request.model` / `gen_ai.response.model`,
`gen_ai.usage.input_tokens` / `output_tokens`, prompt-cache token counts,
`gen_ai.response.finish_reasons`, and the computed cost (`gen_ai.usage.cost`, plus
`gen_ai.usage.cost_usd`). Cache hits are flagged with `sema.gen_ai.cache.hit`. Tool spans
carry `gen_ai.tool.name` / `gen_ai.tool.call.id` / `gen_ai.tool.type`.

### Sessions and users (grouping multi-turn runs)

Every span carries a `gen_ai.conversation.id`, generated per run or supplied by you. For
tools that group by session (such as Langfuse), Sema also emits `session.id` and
`user.id`, so the turns of one conversation appear together:

```sema
(agent/run bot "what is 2 + 3?"  {:session-id "chat-42" :user-id "alice"})
(agent/run bot "now add 10"      {:session-id "chat-42" :user-id "alice"})
;; both runs appear under one session "chat-42", attributed to alice
```

`agent/run`, `llm/chat`, and `llm/complete` accept `:conversation-id`, `:session-id`, and
`:user-id`. If you omit `:session-id` it defaults to the conversation id; a standalone
completion gets a fresh conversation id automatically.

### Metrics

When you export over a network endpoint, Sema also records two standard GenAI metric
histograms:

- `gen_ai.client.token.usage` — token counts (dimension `gen_ai.token.type` = `input` or
  `output`).
- `gen_ai.client.operation.duration` — call latency in seconds.

> Cache hits report zero usage by design (no provider call was made), so token metrics
> undercount real spend when caching is in play.

## Adding your own spans

The `llm/*` and `agent/*` calls are traced for you. When you build your *own* abstraction —
a custom RAG loop, a batch job, a provider Sema doesn't ship — these builtins let it emit
first-class spans too. Every one is a **no-op when tracing is off**, so they are safe to
leave in, and they never change your program's return value.

### Generic spans

```sema
;; with-span runs the body inside a named span carrying an attribute map, ends it on exit
;; (Error status if the body throws), and returns the body's value. Use {} for no attrs.
(with-span "ingest-batch" {:batch.size 100}
  (otel/event "started" {})
  (process-batch))
```

The underlying builtin is `(otel/span name thunk attrs)`; `with-span` is the ergonomic
macro over it. Any LLM/tool spans created inside nest beneath it.

### Annotate the current span

```sema
(otel/set-attribute :http.status 200)         ; one attribute on the innermost span
(otel/set-attributes {:rows 42 :cache.hit true})
(otel/set-status :ok)                          ; or (otel/set-status :error "upstream timeout")
(otel/event "cache-miss" {:key "user:42"})     ; a point-in-time event
```

Attribute values keep their type — integers, floats, and booleans render as numbers/bools
in the backend, not strings.

### Typed spans (render like the built-ins)

For work that *is* an LLM call, tool, or retrieval — but that you implement yourself — use
the typed helpers. They set `gen_ai.operation.name` and, when `SEMA_OTEL_COMPAT` is set,
the backend-native span-kind, so a custom pipeline classifies in Phoenix/Traceloop/Langfuse
exactly like the built-in `llm/*` spans.

```sema
;; A custom LLM/generation call (a provider Sema doesn't natively support):
(otel/llm-span {:model "custom-model" :provider "myco" :operation "chat"}
  (lambda ()
    (let ((resp (my-http-llm-call prompt)))
      ;; Account tokens + cost on the span — same gen_ai.usage.* keys as the built-ins.
      (otel/llm-usage {:input-tokens 120 :output-tokens 30 :cost-usd 0.001})
      resp)))

;; A user-built retrieval step (first-class RETRIEVER span):
(otel/retrieval-span "vector-search" (lambda () (search index query)) {:top-k 5})

;; A user tool:
(otel/tool-span "lookup-weather" (lambda () (weather city)))
```

### Grouping into sessions

`with-session` groups every span started in its body under a session id (and optional
user), filling Langfuse **Sessions/Users** for non-agent code:

```sema
(with-session "chat-42" {:user "alice"}
  (llm/complete "...")        ; inherits session chat-42, user alice
  (my-custom-pipeline))
```

| Form | What it does |
| --- | --- |
| `(with-span name attrs body…)` / `(otel/span name thunk attrs)` | Generic span around a block. |
| `(otel/set-attribute key value)` / `(otel/set-attributes map)` | Set attribute(s) on the innermost active span. |
| `(otel/set-status :ok)` / `(otel/set-status :error msg)` | Set the innermost span's status. |
| `(otel/event name attrs-map)` | Point-in-time event on the current span. |
| `(otel/llm-span config thunk)` + `(otel/llm-usage usage-map)` | Typed LLM/generation span + token/cost accounting. |
| `(otel/tool-span name thunk [attrs])` | Typed TOOL span. |
| `(otel/retrieval-span name thunk [attrs])` | Typed RETRIEVER span. |
| `(with-session id config body…)` / `(otel/with-session id [config] thunk)` | Group spans into a session/user. |

## Privacy

Prompt and response **text** is never recorded unless you explicitly set
`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true`. Token counts, model names,
cost, and timing carry no message text and are always exported. When content capture is
on, long messages are truncated to keep span sizes reasonable.

## Embedding Sema in a Rust application

When Sema runs as a library inside your own program, it **never installs a global tracer
provider on its own** — that is the host application's job. You choose how it connects to
telemetry with `InterpreterBuilder::with_telemetry(mode)`:

```rust
use sema::InterpreterBuilder;
use sema_otel::TelemetryMode;

// Emit against the provider your application already installed in `opentelemetry::global`.
let interp = InterpreterBuilder::new()
    .with_telemetry(TelemetryMode::UseHostGlobal)
    .build();
```

| `TelemetryMode` | Behavior |
| --- | --- |
| `Off` (default) | No telemetry; never touches any global state. |
| `UseHostGlobal` | Emit against the global provider your app already installed (silent no-op if there is none). |
| `OwnProvider(p)` | Emit against a provider you hand to Sema; installs **no** global provider. |
| `FromEnv` | Self-install from the `OTEL_*` / `SEMA_OTEL_FILE` variables. The provider is owned by the built `Interpreter` and flushes when it is dropped. If your app already runs OpenTelemetry, prefer `UseHostGlobal` or `OwnProvider`. |

Sema's spans automatically nest under whatever span is current
(`opentelemetry::Context::current()`), so a host request span becomes the parent of
Sema's `invoke_agent → chat / execute_tool` tree. `Interpreter::new()` and `build()` with
the default `Off` never touch global OpenTelemetry state.
