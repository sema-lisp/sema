---
name: "http/websocket"
module: "web-server"
section: "WebSocket"
---

Handle bidirectional WebSocket connections. Takes a handler function that receives a connection map with `:send`, `:recv`, and `:close` functions.

```sema
(define (handle-ws conn)
  (let ((msg ((:recv conn))))
    (when msg
      ((:send conn) (string/append "echo: " msg))
      (handle-ws conn))))
```

The connection map:

| Key      | Description                                              |
| -------- | -------------------------------------------------------- |
| `:send`  | `(send message)` — Send a string to the client           |
| `:recv`  | `(recv)` — Block until a message arrives, `nil` on close |
| `:close` | `(close)` — Close the connection                         |
