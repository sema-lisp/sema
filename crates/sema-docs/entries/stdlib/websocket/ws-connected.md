---
name: "ws/connected?"
module: "websocket"
section: "WebSocket Client"
params: [{ name: conn, type: connection }]
returns: "bool"
see_also: ["ws/close", "ws/connect", "ws/ping"]
---

Return `true` while the WebSocket connection is open, `false` once it has closed.

```sema
(when (ws/connected? sock)
  (ws/send sock "still here"))
```
