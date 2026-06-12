---
name: "llm/with-rate-limit"
module: "llm"
params: [{ name: rps, type: number }, { name: thunk }]
returns: "any"
---

Run a zero-argument function with a requests-per-second rate limit in effect for LLM calls. The first argument is the allowed rate (requests per second). The previous rate-limit setting is restored after the call. Returns the thunk's result.

```sema
(llm/with-rate-limit 2
  (fn [] (llm/batch ["q1" "q2" "q3"])))
```
