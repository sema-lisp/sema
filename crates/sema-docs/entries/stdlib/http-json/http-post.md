---
name: "http/post"
module: "http-json"
section: "HTTP"
---

```
(http/post url body)
(http/post url body opts)
```

Make an HTTP POST request.

- **url** — string, the request URL
- **body** — request body: a string (sent as-is) or a map (auto-encoded as JSON with `Content-Type: application/json`)
- **opts** — optional map with `:headers` and/or `:timeout`

```sema
;; POST with a map body (auto-JSON-encoded)
(http/post "https://httpbin.org/post"
  {:name "Ada" :age 36})

;; POST with string body and custom headers
(http/post "https://api.example.com/webhook"
  "raw payload"
  {:headers {"Content-Type" "text/plain"}})

;; POST with JSON body and auth
(http/post "https://api.example.com/users"
  {:name "Ada" :role "admin"}
  {:headers {"Authorization" "Bearer tok_abc123"}
   :timeout 10000})
```
