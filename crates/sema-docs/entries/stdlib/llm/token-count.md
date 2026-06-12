---
name: "llm/token-count"
module: "llm"
params: [{ name: input }]
returns: "int"
---

Estimate the number of tokens in a string (or the combined length of a list of strings) using a simple chars-divided-by-4 heuristic. Returns an integer token estimate.

```sema
(llm/token-count "the quick brown fox jumps")   ; => 6
```
