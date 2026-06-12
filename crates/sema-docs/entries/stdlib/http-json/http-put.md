---
name: "http/put"
module: "http-json"
section: "HTTP"
---

```
(http/put url body)
(http/put url body opts)
```

Make an HTTP PUT request. Behaves identically to `http/post` — map bodies are auto-JSON-encoded.

- **url** — string, the request URL
- **body** — request body (string or map)
- **opts** — optional map with `:headers` and/or `:timeout`

```sema
(http/put "https://api.example.com/users/42"
  {:name "Ada Lovelace" :role "admin"})
```
