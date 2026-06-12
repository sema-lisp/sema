---
name: "llm/with-budget"
module: "llm"
params: [{ name: opts, type: map }, { name: thunk }]
returns: "any"
---

Run a zero-argument function under a scoped budget. The opts map requires at least `:max-cost-usd` and/or `:max-tokens`; the scope is pushed before calling the thunk and restored afterward (even on error). LLM calls inside the thunk count against the scoped limit. Returns the thunk's result.

```sema
(llm/with-budget {:max-cost-usd 0.10}
  (fn [] (llm/complete "summarize this in one line")))
```
