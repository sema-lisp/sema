---
name: "http/not-found"
module: "web-server"
section: "Response Helpers"
params: [{ name: body, type: any, doc: "value JSON-encoded into the response body" }]
returns: "map"
see_also: ["http/error", "http/ok", "http/redirect"]
---

Return 404 with a JSON-encoded body.

```sema
(http/not-found {:error "User not found"})
```
