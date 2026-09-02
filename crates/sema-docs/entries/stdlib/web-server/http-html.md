---
name: "http/html"
module: "web-server"
section: "Response Helpers"
params: [{ name: html, type: string }]
returns: "map"
see_also: ["http/text", "http/ok", "http/file"]
---

Build a `200 OK` response whose body is `html` sent as `text/html`. Response
constructors return a plain map with `:status`, `:headers`, and `:body`, so a
handler may also adjust the map (`assoc` a header) before returning it.

The string is sent as is; nothing is escaped or templated. Interpolate values
with `string-append` or an f-string, and escape user-supplied text yourself
before putting it into markup. For JSON use `http/ok`, for plain text
`http/text`, and for a file on disk `http/file`.

```sema
(http/html "<h1>Hello</h1>")
; => {:body "<h1>Hello</h1>" :headers {"content-type" "text/html"} :status 200}

(define (escape-html s)
  (-> s
    (string/replace "&" "&amp;")
    (string/replace "<" "&lt;")
    (string/replace ">" "&gt;")))

(define (handle-home req)
  (http/html (string-append "<h1>Hello, " (escape-html (:name (:query req))) "</h1>")))

;; Add a header to the map the constructor built.
(assoc-in (http/html "<p>x</p>") [:headers "cache-control"] "no-store")
```
