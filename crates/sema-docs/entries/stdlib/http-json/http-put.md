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
- **body** — request body: a map (auto-JSON), string (as-is), or bytevector (raw bytes)
- **opts** — optional map with `:headers`, `:timeout`, `:as` (`:text`/`:bytes`), and/or `:multipart`

```sema
(http/put "https://api.example.com/users/42"
  {:name "Ada Lovelace" :role "admin"})
```
