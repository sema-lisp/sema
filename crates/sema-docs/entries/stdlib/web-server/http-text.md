---
name: "http/text"
module: "web-server"
section: "Response Helpers"
params: [{ name: text, type: string }]
returns: "map"
see_also: ["http/html", "http/ok", "http/file"]
---

Return 200 with `Content-Type: text/plain`.

```sema
(http/text "OK")
```
