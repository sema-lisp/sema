---
name: "llm/with-fallback"
module: "llm"
params: [{ name: providers }, { name: thunk }]
returns: "any"
see_also: ["llm/with-rate-limit", "llm/list-providers", "llm/complete"]
---

Run a zero-argument function with a fallback provider chain in effect: if a call to the active provider fails, the next provider in the list is tried, and so on. The first argument is a list of provider keywords/strings to attempt in order. The previous chain is restored on exit. Returns the thunk's result.

This is the resilience pattern for production agents — survive a provider outage,
rate-limit, or transient error by failing over to a backup. Order the list by
preference: best/cheapest first, a local model (Ollama) last as an always-available
safety net. The chain only kicks in on an *error* from the call; a successful
response (even a bad one) is returned as-is. Every listed provider must already be
configured (`llm/configure`). The error only surfaces if **all** providers in the
chain fail.

```sema
;; Prefer Anthropic, fall back to OpenAI, then a local Ollama model.
(llm/with-fallback [:anthropic :openai :ollama]
  (fn ()
    (llm/complete "draft a one-line tagline for a coffee shop")))
```
