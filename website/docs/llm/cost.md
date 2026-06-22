---
outline: [2, 3]
---

# Cost Tracking & Budgets

## Usage Tracking

### `llm/last-usage`

Get token usage from the most recent LLM call.

```sema
(llm/last-usage)
; => {:prompt-tokens 42 :completion-tokens 15 :total-tokens 57
;     :cache-read-tokens 0 :cache-creation-tokens 0
;     :model "..." :cost-usd 0.0003}
```

### `llm/session-usage`

Get cumulative usage across all LLM calls in the current session.

```sema
(llm/session-usage)
; => {:prompt-tokens 1280 :completion-tokens 410 :total-tokens 1690
;     :cache-read-tokens 1024 :cache-creation-tokens 0 :cost-usd 0.012}
```

#### Prompt-cache tokens

`:cache-read-tokens` and `:cache-creation-tokens` report how many input tokens
were served from (or written to) the provider's **prompt cache** — large savings
when you repeat a stable prefix across calls.

- **OpenAI** and **Gemini** (2.5+) cache *implicitly*: send the same long prefix
  twice and the second call reports `:cache-read-tokens` automatically. Reads are
  a subset of `:prompt-tokens`.
- **Anthropic** reports `:cache-read-tokens` and `:cache-creation-tokens`
  *separately* from `:prompt-tokens` (caching there is opt-in via `cache_control`).
- Providers that don't report cache counts leave these at `0`.

> Cost is currently priced at the standard input rate; cached reads are reported
> for visibility but not yet discounted in `:cost-usd`.

### `llm/reset-usage`

Reset session usage counters.

```sema
(llm/reset-usage)
```

## Pricing Sources

Sema tracks LLM costs using pricing data from these sources, checked in this order:

1. **Custom pricing** — set via `(llm/set-pricing "model" input output)`, always wins
2. **Bundled price list** — a [models.dev](https://models.dev) snapshot (2,400+ models) that ships with Sema, so cost tracking works fully offline with no network calls
3. **Unknown** — if no source matches, cost tracking returns `nil` and budget enforcement is best-effort

The embedded snapshot is refreshed by maintainers with `make update-pricing` and shipped in patch releases. Prices are matched by model id, preferring the canonical first-party listing; when the serving provider is known (e.g. inside an `llm/with-fallback` chain), a reseller/gateway that lists the same model at a different rate is priced correctly.

### `llm/pricing-status`

Check the pricing source and the snapshot date.

```sema
(llm/pricing-status)
; => {:source "embedded" :updated-at "2026-06-18"}
```

## Budget Enforcement

> **Note:** If pricing is unknown for a model (not in any source), budget enforcement operates in best-effort mode — the call proceeds with a one-time warning. Use `(llm/set-pricing)` to set pricing for unlisted models.

### `llm/set-budget`

Set a spending limit (in dollars) for the session. LLM calls that would exceed the budget will fail.

```sema
(llm/set-budget 1.00)   ; set $1.00 spending limit
```

### `llm/budget-remaining`

Check current budget status.

```sema
(llm/budget-remaining)   ; => {:limit 1.0 :spent 0.05 :remaining 0.95}
```

### `llm/with-budget`

Scoped budget — sets spending limits for the duration of a thunk, then restores the previous budget when done. At least one of `:max-cost-usd` or `:max-tokens` is required. When both are provided, **whichever limit is hit first** triggers the error.

```sema
;; Cost-based budget
(llm/with-budget {:max-cost-usd 0.50} (lambda ()
  (llm/complete "Expensive operation")))

;; Token-based budget (useful when pricing is unknown or stale)
(llm/with-budget {:max-tokens 10000} (lambda ()
  (llm/complete "Limited tokens")))

;; Both limits — whichever is reached first stops execution
(llm/with-budget {:max-cost-usd 1.00 :max-tokens 50000} (lambda ()
  (llm/complete "Double-capped")
  (println (format "Budget: ~a" (llm/budget-remaining)))))
```

When a token budget is active, `llm/budget-remaining` includes `:token-limit`, `:tokens-spent`, and `:tokens-remaining` in addition to the cost fields.

#### Streaming and the budget

By default, budgets enforce on **non-streaming** calls (the spend is known after each call completes). A stream's cost isn't known until it ends, so streams aren't budget-gated unless you opt in with `:on-stream :pre-gate` — which refuses to **open** a stream once the scope's spend is already at the cap:

```sema
(llm/with-budget {:max-cost-usd 0.50 :on-stream :pre-gate} (lambda ()
  (llm/stream "..." on-token)))   ; blocked at open once $0.50 is spent
```

A single in-flight stream can still push *past* the cap (you only learn its cost when it finishes), but the next call is blocked. Usage is tracked either way.

### `llm/clear-budget`

Remove the spending limit.

```sema
(llm/clear-budget)
```

### `llm/set-pricing`

Set custom pricing for a model (overrides both dynamic and built-in pricing). Costs are per million tokens.

```sema
(llm/set-pricing "my-model" 1.0 3.0)   ; $1.00/M input, $3.00/M output
```

## Batch & Parallel

### `llm/batch`

Send multiple prompts concurrently and collect all results.

```sema
(llm/batch (list "Translate 'hello' to French"
                 "Translate 'hello' to Spanish"
                 "Translate 'hello' to German"))
```

### `llm/pmap`

Map a function over items, sending all resulting prompts in parallel.

```sema
(llm/pmap
  (fn (word) (format "Define: ~a" word))
  '("serendipity" "ephemeral" "ubiquitous")
  {:max-tokens 50})
```
