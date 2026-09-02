---
name: "http/html"
module: "web-server"
section: "Response Helpers"
params: [{ name: html, type: string }]
returns: "map"
see_also: ["http/text", "http/ok", "http/file"]
---

Return 200 with `Content-Type: text/html`.

```sema
(http/html "<h1>Hello</h1><p>Welcome to Sema.</p>")
```
