---
outline: [2, 3]
---

# LLM Primitives

Sema's differentiating feature: LLM operations are first-class language primitives with prompts, conversations, tools, and agents as native data types.

## Setup

Set one or more API keys as environment variables:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
export OPENAI_API_KEY=sk-...
export DEEPSEEK_API_KEY=...
export TOGETHER_API_KEY=...
export FIREWORKS_API_KEY=...
# or any other supported provider
```

Sema auto-detects and configures all available providers on startup. Use `--no-llm` to skip auto-configuration.

See [Provider Management](/docs/llm/providers) for the full list of supported providers and configuration options.

## Features

### [Completion & Chat](/docs/llm/completion)

Simple completions, multi-message chat, and streaming responses.

### [Prompts & Messages](/docs/llm/prompts)

Prompts as composable s-expressions, message construction, and prompt inspection.

### [Conversations](/docs/llm/conversations)

Persistent, immutable conversation state with automatic LLM round-trips.

### [Tools & Agents](/docs/llm/tools-agents)

Define tools the LLM can invoke, and build agents with system prompts, tools, and multi-turn loops.

### [Embeddings & Similarity](/docs/llm/embeddings)

Generate embeddings (as bytevectors), compute cosine similarity, and access embedding dimensions.

### [Structured Extraction](/docs/llm/extraction)

Extract structured data from text and images, classify inputs, and work with multi-modal content.

### [Vector Store & Math](/docs/llm/vector-store)

In-memory vector store for semantic search, plus vector math utilities (cosine similarity, dot product, normalize, distance).

### [Caching](/docs/llm/caching)

In-memory LLM response caching for iterative development and deduplication.

### [Cassettes (Record & Replay)](/docs/llm/cassettes)

Record real LLM/agent responses to a file once, then replay them deterministically — keyless, offline tests and reproducible demos.

### [Resilience & Retry](/docs/llm/resilience)

Fallback provider chains, rate limiting, generic retry with exponential backoff, and convenience functions (`llm/summarize`, `llm/compare`).

### [Provider Management](/docs/llm/providers)

Auto-configuration, runtime provider switching, custom providers, and OpenAI-compatible endpoints.

### [Cost Tracking & Budgets](/docs/llm/cost)

Usage tracking, budget enforcement, and batch/parallel operations.

### [Workflows](/docs/llm/workflows)

Define multi-phase agent pipelines as ordinary Sema code. Every step is
journaled to a frozen JSONL run directory — resume, replay, or fork without
losing state. Budget caps, parallel/pipeline fan-out, and a live web viewer.

### Observability (OpenTelemetry)

Built-in, standards-compliant OpenTelemetry tracing + metrics for **every** LLM and
agent run — no manual instrumentation. Each completion and tool call is auto-traced
(`invoke_agent → chat → execute_tool`) with tokens, cost, and latency, exportable to
any OTLP backend or a JSONL file — turned on with one environment variable or an
`otel/configure` call. Off by default, zero-cost when off.

- **[Tracing & Metrics](/docs/llm/observability)** — the GenAI spans and metrics, sessions,
  privacy controls, and embedding Sema in your own app.
- **[Backend Compatibility](/docs/llm/otel-compat)** — label the data so tools that use their
  own attribute names (Arize Phoenix, Langfuse, Traceloop, LangSmith) read it too via
  `SEMA_OTEL_COMPAT`. Most other tools work with no extra setup.
