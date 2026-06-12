---
name: "http/error"
module: "web-server"
section: "Response Helpers"
---

Return a custom status code with a JSON-encoded body.

```sema
(http/error 422 {:errors ["Invalid email" "Name required"]})
(http/error 503 {:error "Service unavailable"})
```
