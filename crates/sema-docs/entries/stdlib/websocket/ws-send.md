---
name: "ws/send"
module: "websocket"
section: "WebSocket Client"
params: [{ name: conn, type: connection }, { name: message, type: any }]
returns: "nil"
---

Send a message over a WebSocket connection. A string sends a text frame, a bytevector a binary frame, a map is sent as JSON text, and explicit framing selects the type.

```sema
(ws/send sock "hello")                 ;; text
(ws/send sock (bytevector 1 2 3))      ;; binary
(ws/send sock {:type "chat" :body 1})  ;; JSON text
(ws/send sock {:text "hi"})            ;; explicit text
(ws/send sock {:binary (bytevector 9)});; explicit binary
(ws/send sock {:json {:a 1}})          ;; explicit JSON
```
