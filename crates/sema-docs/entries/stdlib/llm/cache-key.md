---
name: "llm/cache-key"
module: "llm"
params: [{ name: prompt, type: string }, { name: opts, type: map }]
returns: "string"
see_also: ["llm/with-cache", "llm/cache-stats", "llm/cache-clear"]
---

Compute the cache key (a hex digest) that the response cache would use for a given prompt and options. The opts map accepts `:model`, `:temperature`, and `:system`. Useful for inspecting or pre-seeding the cache.

The key folds in the prompt **and** the options, so changing the model,
temperature, or system prompt yields a different key — i.e. caching is per
exact-request, and two calls only share a cached response when all of these match.

```sema
(llm/cache-key "hello" {:model "gpt-4o-mini"})
; => "e8b31aa2d7a4b92427d28a5d3ea1c3246450b203e2874cf28c35c1ca3c1f0ade"

;; Different model -> different key -> separate cache entry.
(llm/cache-key "hello" {:model "gpt-4o"})
; => "...a different digest..."
```
