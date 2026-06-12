---
name: "http/router"
module: "web-server"
section: "Routing"
---

Create a handler function from a list of route definitions. Each route is a vector of `[method pattern handler]`.

```sema
(define routes
  [[:get  "/"            handle-home]
   [:get  "/users/:id"   handle-user]
   [:post "/users"       handle-create]
   [:any  "/echo"        handle-echo]])

(define app (http/router routes))
(http/serve app {:port 3000})
```

Supported methods: `:get`, `:post`, `:put`, `:patch`, `:delete`, `:any` (matches all methods), `:ws` (WebSocket upgrade), and `:static` (static file directory).

Routes are matched top-to-bottom — first match wins. Unmatched routes return 404.
