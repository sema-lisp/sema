---
name: "http/delete"
module: "http-json"
section: "HTTP"
---

```
(http/delete url)
(http/delete url opts)
```

Make an HTTP DELETE request.

- **url** — string, the request URL
- **opts** — optional map with `:headers`, `:timeout`, and/or `:as` (`:text`/`:bytes`)

```sema
(http/delete "https://api.example.com/users/42"
  {:headers {"Authorization" "Bearer tok_abc123"}})
```
