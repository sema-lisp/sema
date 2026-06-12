---
name: "route/prefix"
module: "route"
params: [{ name: prefix, type: string }, { name: routes }]
returns: "list"
---

Prepend a path prefix to every route's pattern in the given list, returning a new list of routes. Each route is a `[method pattern handler ...]` vector; a trailing slash on the prefix is trimmed.

```sema
(route/prefix "/api"
  [[:get "/users" list-users]
   [:post "/users" create-user]])
```
