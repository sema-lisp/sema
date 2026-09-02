---
name: "log/error"
module: "log"
params: [{ name: args }]
returns: "nil"
see_also: ["log/debug", "log/info", "log/warn", "spy"]
---

Write an `[ERROR]` log line to stderr. Accepts one or more values, joined by spaces (strings as-is, others stringified). Any active logging context is appended.

```sema
(log/error "failed to connect:" err)
```
