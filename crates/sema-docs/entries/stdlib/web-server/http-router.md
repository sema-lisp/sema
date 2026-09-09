---
name: "http/router"
module: "web-server"
section: "Routing"
params: [{ name: routes, type: "list | vector" }]
returns: "function"
see_also: ["http/serve", "route/prefix", ":static", "tools->routes"]
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

Request paths are split into segments before percent-decoding. Parameters and
wildcard captures contain decoded UTF-8 text; a literal `+` remains `+`.
An encoded slash stays inside its segment and does not match an extra route
segment. Invalid percent escapes or invalid UTF-8 do not match. Static routes
also reject decoded slashes, backslashes, and NUL bytes before file lookup.

`http/serve` requires a validated WebSocket upgrade before invoking a `:ws`
handler. Ordinary HTTP requests to that route receive 400 without running it.
