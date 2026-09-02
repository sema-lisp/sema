---
name: "http/put"
module: "http-json"
section: "HTTP"
params: [{ name: url, type: string }, { name: body, type: any, doc: "map (JSON), string, or bytevector" }, { name: opts, type: map, doc: "optional :headers/:timeout/:as/:multipart" }]
returns: "map"
see_also: ["http/post", "http/delete", "http/request"]
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
