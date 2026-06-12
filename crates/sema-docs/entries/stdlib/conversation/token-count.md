---
name: "conversation/token-count"
module: "conversation"
params: [{ name: conv, type: conversation }]
returns: "int"
---

Estimate the total number of tokens across all message contents in the conversation, using a ~4-characters-per-token heuristic.

```sema
(conversation/token-count conv)   ; => 312
```
