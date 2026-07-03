---
name: "ws/listen"
module: "websocket"
section: "WebSocket Client"
---

Drive an evented receive loop over a connection, dispatching each frame to the matching handler. All handlers are optional: `:on-open` `(conn)`, `:on-message` `(conn msg)` where `msg` is a text string or binary bytevector, `:on-close` `(conn info)` where `info` is `{:code :reason}`, and `:on-error` `(conn err)`. On native it spawns an async task and returns its promise to await; in the browser it wires the socket's events directly.

```sema
(with-open (sock (ws/connect "wss://stream.example.com"))
  (async/await
    (ws/listen sock
      {:on-message (fn (c m) (println m))
       :on-close   (fn (c info) (println "closed" (:code info)))})))
```
