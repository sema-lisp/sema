---
name: "log/info"
module: "log"
params: [{ name: args }]
returns: "nil"
---

Write an `[INFO]` log line to stderr. Accepts one or more values, joined by spaces (strings as-is, others stringified). Any active logging context is appended.

```sema
(log/info "server started on port" 8080)
```
