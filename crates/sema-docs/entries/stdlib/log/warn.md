---
name: "log/warn"
module: "log"
params: [{ name: args }]
returns: "nil"
---

Write a `[WARN]` log line to stderr. Accepts one or more values, joined by spaces (strings as-is, others stringified). Any active logging context is appended.

```sema
(log/warn "retrying request, attempt" n)
```
