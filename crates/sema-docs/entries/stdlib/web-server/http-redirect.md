---
name: "http/redirect"
module: "web-server"
section: "Response Helpers"
params: [{ name: url, type: string }]
returns: "map"
see_also: ["http/not-found", "http/ok", "http/router"]
---

Build a `302 Found` response with a `Location` header pointing at `url` and
an empty body. The browser follows it with a `GET`, which is what a handler
wants after processing a form submission (the post/redirect/get pattern) or
when sending an unauthenticated visitor to a login page.

`url` may be absolute or a path on the same origin. Only `302` is built here;
for a permanent redirect or another status, return the map with `:status`
changed, as shown below.

```sema
(http/redirect "/login")
; => {:body "" :headers {"location" "/login"} :status 302}

(define (handle-submit req)
  (save-form! (:form req))
  (http/redirect "/thanks"))

;; A permanent redirect: same shape, different status.
(assoc (http/redirect "https://example.com/new") :status 301)
```
