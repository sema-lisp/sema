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
- **body** — request body: a map (auto-encoded as JSON with `Content-Type: application/json`), a string (sent as-is), or a bytevector (sent as raw bytes). Ignored when `:multipart` is set.
- **opts** — optional map with `:headers`, `:timeout`, `:as` (`:text` (default) or `:bytes` for a bytevector response body), and/or `:multipart` (a list of `{:name .. :content <string|bytevector> :filename ..? :content-type ..?}` part maps for `multipart/form-data` uploads)

```sema
;; POST with a map body (auto-JSON-encoded)
(http/post "https://httpbin.org/post"
  {:name "Ada" :age 36})

;; POST with string body and custom headers
(http/post "https://api.example.com/webhook"
  "raw payload"
  {:headers {"Content-Type" "text/plain"}})

;; Upload raw bytes; receive the response body as a bytevector
(http/post "https://api.example.com/upload"
  (file/read-bytes "photo.jpg")
  {:headers {"Content-Type" "image/jpeg"} :as :bytes})

;; Multipart file upload (positional body ignored when :multipart is set)
(http/post "https://api.example.com/documents"
  {}
  {:multipart (list
     {:name "purpose" :content "rag-ingest"}
     {:name "file" :filename "report.pdf"
      :content (file/read-bytes "report.pdf")
      :content-type "application/pdf"})})
```
