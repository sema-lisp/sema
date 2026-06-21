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
# or any other supported provider
```

Sema auto-detects and configures all available providers on startup. Use `--no-llm` to skip auto-configuration.

See [Provider Management](./providers.md) for the full list of supported providers and configuration options.

## Features

### [Completion & Chat](./completion.md)

Simple completions, multi-message chat, and streaming responses.

### [Prompts & Messages](./prompts.md)

Prompts as composable s-expressions, message construction, and prompt inspection.

### [Conversations](./conversations.md)

Persistent, immutable conversation state with automatic LLM round-trips.

### [Tools & Agents](./tools-agents.md)

Define tools the LLM can invoke, and build agents with system prompts, tools, and multi-turn loops.

### [Embeddings & Similarity](./embeddings.md)

Generate embeddings (as bytevectors), compute cosine similarity, and access embedding dimensions.

### [Structured Extraction](./extraction.md)

Extract structured data from text and images, classify inputs, and work with multi-modal content.

### [Vector Store & Math](./vector-store.md)

In-memory vector store for semantic search, plus vector math utilities (cosine similarity, dot product, normalize, distance).

### [Caching](./caching.md)

In-memory LLM response caching for iterative development and deduplication.

### [Resilience & Retry](./resilience.md)

Fallback provider chains, rate limiting, generic retry with exponential backoff, and convenience functions (`llm/summarize`, `llm/compare`).

### [Provider Management](./providers.md)

Auto-configuration, runtime provider switching, custom providers, and OpenAI-compatible endpoints.

### [Cost Tracking & Budgets](./cost.md)

Usage tracking, budget enforcement, and batch/parallel operations.

### [Observability (OpenTelemetry)](./observability.md)

Standards-compliant OpenTelemetry traces + metrics (GenAI semantic conventions) for
every LLM/agent run — export to Jaeger, Langfuse, Datadog, Grafana, or a JSONL file.
Off by default, zero-cost when off.
