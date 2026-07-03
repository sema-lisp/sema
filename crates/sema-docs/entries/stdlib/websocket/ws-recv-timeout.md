---
name: "ws/recv-timeout"
module: "websocket"
section: "WebSocket Client"
params: [{ name: conn, type: connection }, { name: timeout-ms, type: int }]
returns: "map"
---

Like `ws/recv`, but returns the `:timeout` keyword if no frame arrives within `timeout-ms` milliseconds (distinct from `nil`, which means the connection closed). Native only.

```sema
(match (ws/recv-timeout sock 2000)
  :timeout    (println "no message in 2s")
  {:text t}   (println t)
  nil         (println "closed"))
```
