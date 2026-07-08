---
name: "http/request"
module: "http-json"
section: "HTTP"
---

```
(http/request method url)
(http/request method url opts)
(http/request method url opts body)
```

Make an HTTP request with any method. Use this for methods not covered by the convenience functions (e.g., `PATCH`, `HEAD`).

- **method** — string, HTTP method (case-insensitive, converted to uppercase). Supported: `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`
- **url** — string, the request URL
- **opts** — optional map with `:headers`, `:timeout`, `:as` (`:text` (default) or `:bytes` for a bytevector response body), and/or `:multipart` (a list of part maps for `multipart/form-data` uploads)
- **body** — optional request body: a map (auto-JSON), string (as-is), or bytevector (raw bytes)

```sema
;; PATCH request
(http/request "PATCH" "https://api.example.com/users/42"
  {:headers {"Content-Type" "application/json"}}
  {:name "Updated Name"})

;; HEAD request (body will be empty)
(define resp (http/request "HEAD" "https://example.com"))
(:status resp)    ; => 200
(:body resp)      ; => ""
```
