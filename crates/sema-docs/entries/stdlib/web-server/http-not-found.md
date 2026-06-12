---
name: "http/not-found"
module: "web-server"
section: "Response Helpers"
---

Return 404 with a JSON-encoded body.

```sema
(http/not-found {:error "User not found"})
```
