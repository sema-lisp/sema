---
name: "ws/close"
module: "websocket"
section: "WebSocket Client"
params: [{ name: conn, type: connection }]
returns: "nil"
---

Close a WebSocket connection. A `with-open` binding closes the connection automatically on exit, so an explicit `ws/close` is only needed for manually managed connections.

```sema
(ws/close sock)
```
