---
name: "llm/reset-usage"
module: "llm"
params: []
returns: "nil"
---

Reset session usage tracking: zeros the accumulated prompt/completion token counts, clears the last-usage record, and resets session cost to zero.

```sema
(llm/reset-usage)
```
