---
name: "llm/reset-usage"
module: "llm"
params: []
returns: "nil"
see_also: ["llm/session-usage", "llm/last-usage"]
syntax: "(llm/reset-usage)"
---

Reset session usage tracking: zeros the accumulated prompt/completion token counts, clears the last-usage record, and resets session cost to zero.

```sema
(llm/reset-usage)
```
