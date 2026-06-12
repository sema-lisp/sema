---
name: "log/debug"
module: "log"
params: [{ name: args }]
returns: "nil"
---

Write a `[DEBUG]` log line to stderr. Accepts one or more values, joined by spaces (strings as-is, others stringified). Any active logging context is appended.

```sema
(log/debug "request payload" payload)
```
