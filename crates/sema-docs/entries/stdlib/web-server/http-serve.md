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

| Option           | Default     | Description                                                         |
| ---------------- | ----------- | ------------------------------------------------------------------- |
| `:port`          | `3000`      | TCP port to bind                                                    |
| `:host`          | `"0.0.0.0"` | Address to bind to                                                  |
| `:port-fallback` | `false`     | If the port is taken, bind the next free port instead of failing    |
| `:on-listen`     | —           | Function called once bound with `{:host :port :url}`                |

The handler is any function `(request-map -> response-map)`. This can be a plain function, a router, or a middleware-wrapped stack.

### Automatic port fallback

By default `http/serve` fails fast when the port is already in use. Pass
`:port-fallback true` to walk to the next free port instead — handy for dev
servers and multiple instances. Because the actual port may then differ from the
one requested, use `:on-listen` to learn where the server ended up:

```sema
(http/serve handler
  {:port 3000
   :port-fallback true
   :on-listen (fn (info)
     (println (string-append "Ready at " (:url info))))})
```

`:on-listen` runs once, on the main thread, right after the socket binds and
before the request loop starts.
