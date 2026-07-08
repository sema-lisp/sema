---
name: "http/get"
module: "http-json"
section: "HTTP"
params: [{ name: url, type: string }, { name: opts, type: map, doc: "optional :headers/:timeout/:as" }]
returns: "map"
---

```
(http/get url)
(http/get url opts)
```

Make an HTTP GET request.

- **url** — string, the request URL
- **opts** — optional map with `:headers`, `:timeout`, and/or `:as` (`:text` (default) or `:bytes` to receive the response body as a bytevector — for binary downloads)

```sema
;; Simple GET
(http/get "https://httpbin.org/get")

;; GET with custom headers
(http/get "https://api.example.com/users"
  {:headers {:authorization "Bearer my-token"}})

;; Download binary data as a bytevector and save it
(let ((resp (http/get "https://example.com/image.png" {:as :bytes})))
  (file/write-bytes "image.png" (:body resp)))
```
