---
name: "ws/connect"
module: "websocket"
section: "WebSocket Client"
params: [{ name: url, type: string }, { name: opts, type: map, doc: "optional :headers/:subprotocols/:timeout/:retries/:retry-backoff-ms" }]
returns: "connection"
---

Open a WebSocket connection to a `ws://` or `wss://` server and return a connection handle.

- **url** — the `ws://`/`wss://` endpoint
- **opts** — optional map: `:headers`, `:subprotocols`, `:timeout` (handshake), `:retries` and `:retry-backoff-ms` (exponential backoff). In the browser build only `:subprotocols` applies.

```sema
(define sock (ws/connect "wss://echo.websocket.org"))

;; with options
(ws/connect "wss://api.example.com/stream"
  {:subprotocols ["v1.chat"] :timeout 5000 :retries 3})
```
