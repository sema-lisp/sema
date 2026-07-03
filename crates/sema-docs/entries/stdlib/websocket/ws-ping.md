---
name: "ws/ping"
module: "websocket"
section: "WebSocket Client"
params: [{ name: conn, type: connection }, { name: payload, type: any, doc: "optional string or bytevector" }]
returns: "nil"
---

Send a ping frame to keep the connection alive; the peer answers with a pong automatically. An optional string or bytevector payload is echoed in the pong.

```sema
(ws/ping sock)
(ws/ping sock "keepalive")
```
