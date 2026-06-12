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
- **opts** — optional map with `:headers` and/or `:timeout`
- **body** — optional request body (string or map)

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
