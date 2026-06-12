---
name: "tools->routes"
module: "web-server"
section: "Routing"
params: [{ name: tools, type: "list | vector" }]
---

Convert a list of tool definitions (from `deftool`) into HTTP routes. Each tool gets a `POST /tools/<name>` endpoint that runs the tool handler against the request's JSON body, plus a `/tools/<name>/schema` endpoint exposing its parameter schema.

```sema
(tools->routes [my-tool other-tool])
; => a list of route handlers for use with the web server
```
