---
name: "http/query"
module: "http-json"
section: "HTTP"
---

```
(http/query url body)
(http/query url body opts)
```

Make an HTTP QUERY request ([RFC 10008](https://datatracker.ietf.org/doc/rfc10008/)). QUERY is safe and idempotent like `GET`, but carries a request body like `POST` — for queries too large or structured to fit in the URL. The server processes the enclosed content and returns the result.

- **url** — string, the request URL
- **body** — request body: a map (auto-encoded as JSON), a string (sent as-is), or a bytevector (raw bytes)
- **opts** — optional map with `:headers`, `:timeout`, and/or `:as` (`:text` (default) or `:bytes`)

```sema
;; Send a structured query in the body instead of the URL
(let ((resp (http/query "https://api.example.com/search"
              {:filter {:status "active"} :limit 50})))
  (json/decode (:body resp)))
```

The web server matches QUERY requests with a `:query` route:

```sema
(http/router
  (list (vector :query "/search"
          (fn (req) (http/ok (run-search (json/decode (:body req))))))))
```
