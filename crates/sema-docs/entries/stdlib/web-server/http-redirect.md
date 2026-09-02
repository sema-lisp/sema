---
name: "http/redirect"
module: "web-server"
section: "Response Helpers"
params: [{ name: url, type: string }]
returns: "map"
see_also: ["http/not-found", "http/ok", "http/router"]
---

Return a 302 redirect to a URL.

```sema
(http/redirect "https://example.com/login")
```
