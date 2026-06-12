---
name: "llm/with-cache"
module: "llm"
params: [{ name: opts, type: map }, { name: thunk }]
returns: "any"
---

Run a zero-argument function with LLM response caching enabled for its duration. With two arguments, the first is an opts map accepting `:ttl` (cache time-to-live in seconds, default 3600); with one argument it is just the thunk. The previous cache settings are restored after the call. Returns the thunk's result.

```sema
(llm/with-cache {:ttl 600}
  (fn [] (llm/complete "what is 2+2?")))
```
