---
name: "ws/connected?"
module: "websocket"
section: "WebSocket Client"
params: [{ name: conn, type: connection }]
returns: "bool"
---

Return `true` while the WebSocket connection is open, `false` once it has closed.

```sema
(when (ws/connected? sock)
  (ws/send sock "still here"))
```
