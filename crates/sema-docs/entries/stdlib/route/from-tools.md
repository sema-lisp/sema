---
name: "route/from-tools"
module: "route"
params: [{ name: tools }]
returns: "list"
see_also: ["tools->routes", "route/prefix", "http/router"]
---

Generate HTTP routes from a list of tool definitions. For each tool it creates a `POST /tools/<name>` route whose handler runs the tool with the request's JSON body and returns the result as JSON, plus a `GET /tools/<name>/schema` route exposing the tool's name, description, and parameter schema. Canonical alias for `tools->routes`.

```sema
(http/serve {:port 8080
             :routes (route/from-tools [get-weather search-docs])})
```
