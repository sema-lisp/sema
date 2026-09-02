---
name: "http/created"
module: "web-server"
section: "Response Helpers"
params: [{ name: body, type: any, doc: "value JSON-encoded into the response body" }]
returns: "map"
see_also: ["http/ok", "http/error", "http/no-content"]
---

Return 201 with a JSON-encoded body.

```sema
(http/created {:id 42 :name "Ada"})
```
