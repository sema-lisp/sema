---
name: "http/serve"
module: "web-server"
section: "Serving"
---

Start an HTTP server. Takes a handler function and an optional options map. The handler receives a request map and returns a response map. This function blocks — it becomes the server's run loop.

```sema
(http/serve handler)
(http/serve handler {:port 3000})
(http/serve handler {:port 8080 :host "127.0.0.1"})
```

| Option  | Default     | Description        |
| ------- | ----------- | ------------------ |
| `:port` | `3000`      | TCP port to bind   |
| `:host` | `"0.0.0.0"` | Address to bind to |

The handler is any function `(request-map -> response-map)`. This can be a plain function, a router, or a middleware-wrapped stack.
