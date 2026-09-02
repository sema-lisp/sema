---
name: "http/error"
module: "web-server"
section: "Response Helpers"
params: [{ name: status, type: int }, { name: body, type: any, doc: "value JSON-encoded into the response body" }]
returns: "map"
see_also: ["http/not-found", "http/ok", "http/text"]
---

Return a custom status code with a JSON-encoded body.

```sema
(http/error 422 {:errors ["Invalid email" "Name required"]})
(http/error 503 {:error "Service unavailable"})
```
