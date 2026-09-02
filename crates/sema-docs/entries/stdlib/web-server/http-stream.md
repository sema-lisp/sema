---
name: "http/stream"
module: "web-server"
section: "SSE Streaming"
params: [{ name: handler, type: function, doc: "called with a send callback" }]
returns: "map"
see_also: ["http/websocket", "http/serve", "http/ok"]
---

Return a Server-Sent Events stream. Takes a handler function that receives a `send` callback.

```sema
(define (handle-events req)
  (http/stream
    (fn (send)
      (send "connected")
      (sleep 1000)
      (send "update 1")
      (sleep 1000)
      (send "update 2"))))
```

The stream stays open as long as the handler is running. When the handler returns, the stream closes.

```sema
;; Route it like any other handler
(define routes
  [[:get "/events" handle-events]])
```

```bash
$ curl -N http://localhost:3000/events
data: connected

data: update 1

data: update 2
```
