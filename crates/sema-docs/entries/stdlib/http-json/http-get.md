---
name: "http/get"
module: "http-json"
section: "HTTP"
params: [{ name: url, type: string }, { name: opts, type: map, doc: "optional :headers/:timeout" }]
returns: "map"
---

```
(http/get url)
(http/get url opts)
```

Make an HTTP GET request.

- **url** — string, the request URL
- **opts** — optional map with `:headers` and/or `:timeout`

```sema
;; Simple GET
(http/get "https://httpbin.org/get")

;; GET with custom headers
(http/get "https://api.example.com/users"
  {:headers {:authorization "Bearer my-token"}})
```
