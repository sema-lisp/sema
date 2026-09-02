---
name: "log/info"
module: "log"
params: [{ name: args }]
returns: "nil"
see_also: ["log/debug", "log/warn", "log/error", "spy"]
---

Write an `[INFO]` log line to stderr for normal operational events. Accepts one or more values, joined by spaces (strings as-is, others stringified). Any active logging context is appended.

```sema
(log/info "server started on port" 8080)
; stderr: [INFO] server started on port 8080
```

All four levels — `log/debug`, `log/info`, `log/warn`, `log/error` — share the same shape and write to **stderr** (so they don't pollute stdout pipelines). Pick the level by severity: `debug` for noisy detail, `info` for normal events, `warn` for recoverable trouble, `error` for failures.

Key/value context set with `context/set` is appended automatically as a map, so you don't have to thread it through every call:

```sema
(context/set :request-id "abc-123")
(log/info "handling request")
; stderr: [INFO] handling request {:request-id "abc-123"}
```
