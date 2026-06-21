---
outline: [2, 3]
---

# Resilience & Retry

## Fallback Provider Chains

### `llm/with-fallback`

Wraps a thunk with a fallback chain of providers. If the LLM call fails with one provider, automatically tries the next provider in the list.

```sema
(llm/with-fallback [:anthropic :openai :groq]
  (lambda () (llm/complete "Hello")))
```

#### Model selection across the chain

Model ids are provider-specific (a Claude id is meaningless to OpenAI), so each chain entry resolves its own model:

- A **bare provider keyword** (e.g. `:anthropic`) uses that provider's [default model](./providers#default-models), or whatever you set via `(llm/configure :anthropic {:default-model "..."})`. This is the recommended form — leave the body's `(llm/complete ...)` **unpinned** so every provider gets a model id valid for itself.
- If the body pins a `:model`, that exact string is sent to **every** provider in the chain. That's fine for a homogeneous chain, but pinning a provider-specific id (e.g. a Claude model) will fail on any other provider it falls back to.

#### Per-provider model overrides

To target a different model per provider within a single chain, give chain entries as `[provider model]` pairs or `{:provider :model}` maps. A per-provider override **wins over any `:model` pinned in the body**:

```sema
;; Anthropic uses Opus, OpenAI uses GPT-5.5, Groq uses its default
(llm/with-fallback [[:anthropic "claude-opus-4-8"]
                    [:openai    "gpt-5.5"]
                    :groq]
  (lambda () (llm/complete "Hello")))

;; Map form is equivalent and lets you omit :model to use the provider default
(llm/with-fallback [{:provider :anthropic :model "claude-opus-4-8"}
                    {:provider :openai}]
  (lambda () (llm/complete "Hello")))
```

## Automatic Retry on Transient Errors

LLM calls (`llm/complete`, `llm/chat`, `agent/run`, and the fallback-chain path)
**automatically retry transient failures** — no configuration needed:

- Retried: HTTP 429 (rate limited), 5xx server errors, and network/timeout errors.
- Not retried: 4xx client errors other than 429 (e.g. 400 bad request), and parse
  errors — these won't succeed on a retry, so they fail fast.
- Backoff: capped **exponential backoff with full jitter** (base 500ms, doubling
  per attempt, capped at 30s), up to 3 retries. A 429 honors the provider's
  `retry-after` hint when present.

This is distinct from [`llm/with-fallback`](#fallback-provider-chains) (which
switches *providers* on failure) and the generic [`retry`](#generic-retry) (which
wraps *any* thunk). They compose: each provider in a fallback chain does its own
transient-error retry before the chain moves on.

::: tip Streaming bypass
Streaming calls (`llm/stream`) currently bypass the automatic retry, response
cache, budget enforcement, and fallback chain — they hit the provider directly.
Use the non-streaming forms when you need those guarantees.
:::

## Rate Limiting

### `llm/with-rate-limit`

Wraps a thunk with token-bucket rate limiting. Takes a rate (requests per second) and a thunk. Useful to avoid hitting API rate limits.

```sema
(llm/with-rate-limit 5 (lambda () (llm/complete "Hello")))
```

## Generic Retry

### `retry`

Retries a thunk on failure with exponential backoff. Takes a thunk and an optional options map.

```sema
;; Default: 3 attempts, 100ms base delay, 2.0 backoff
(retry (lambda () (http/get "https://example.com")))

;; Custom options
(retry (lambda () (http/get "https://example.com"))
  {:max-attempts 5 :base-delay-ms 200 :backoff 1.5})
```

Options:

| Key              | Type    | Default | Description                        |
| ---------------- | ------- | ------- | ---------------------------------- |
| `:max-attempts`  | integer | 3       | Maximum number of attempts         |
| `:base-delay-ms` | integer | 100     | Initial delay between retries (ms) |
| `:backoff`       | float   | 2.0     | Backoff multiplier                 |

> **Note:** `retry` is in the stdlib (not LLM-specific) — it works with any function.

## LLM Convenience Functions

### `llm/summarize`

Summarize text using an LLM. Takes text and an optional options map.

```sema
(llm/summarize "Long article text here...")
(llm/summarize "Long text" {:model "claude-haiku-4-5-20251001" :max-tokens 200})
```

### `llm/compare`

Compare two texts using an LLM. Takes two strings and an optional options map.

```sema
(llm/compare "Text A" "Text B")
(llm/compare "Text A" "Text B" {:model "claude-haiku-4-5-20251001"})
```
