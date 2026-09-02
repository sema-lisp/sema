---
name: "llm/with-cache"
module: "llm"
params: [{ name: opts, type: map }, { name: thunk }]
returns: "any"
see_also: ["llm/cache-stats", "llm/cache-clear", "llm/cache-key"]
---

Run a zero-argument function with LLM response caching enabled for its duration, so identical requests (same prompt + model + temperature + system) reuse the first response instead of calling the provider again. With two arguments the first is an opts map accepting `:ttl` (cache time-to-live in seconds, default 3600); with one argument it is just the thunk. The previous cache settings are restored on exit. Returns the thunk's result.

A cache hit makes no provider call, so it costs nothing and reports **zero
usage** — it doesn't burn budget (`llm/with-budget`) or appear in
`llm/session-usage`. That makes `with-cache` the natural way to make a script
idempotent and cheap to re-run during development: the first run pays, every
re-run is free until the TTL expires. The cache key is exact-match (see
`llm/cache-key`), so a one-character prompt change is a fresh request. Inspect
hit/miss counts with `llm/cache-stats` and wipe it with `llm/cache-clear`.

For *deterministic* test fixtures (commit the responses, replay with no network
and no key) prefer `llm/with-cassette` — caching is opportunistic and TTL-bound,
a cassette is a pinned recording.

```sema
;; Second call to the same prompt is served from cache — one provider call total.
(llm/with-cache {:ttl 600}
  (fn ()
    (let [a (llm/complete "what is 2+2?")
          b (llm/complete "what is 2+2?")]   ; cache hit, free
      (list a b))))
```
