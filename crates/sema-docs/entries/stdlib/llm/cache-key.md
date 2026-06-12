---
name: "llm/cache-key"
module: "llm"
params: [{ name: prompt, type: string }, { name: opts, type: map }]
returns: "string"
---

Compute the cache key string that would be used for a given prompt and options. The opts map accepts `:model`, `:temperature`, and `:system`. Useful for inspecting or pre-seeding the response cache.

```sema
(llm/cache-key "hello" {:model "gpt-4o-mini"})   ; => "a1b2c3..."
```
