---
name: "http/not-found"
module: "web-server"
section: "Response Helpers"
params: [{ name: body, type: any, doc: "value JSON-encoded into the response body" }]
returns: "map"
see_also: ["http/error", "http/ok", "http/redirect"]
---

Build a `404 Not Found` response. `body` is JSON-encoded, with
`Content-Type: application/json`, so a map gives a structured error object
and a string gives a JSON string literal (with quotes). The argument is
required; pass `nil` for `null`.

Use it from a handler when a looked-up resource does not exist. Requests that
match no route at all are answered by `http/router` on its own. `http/error`
is the general form for any status code with a JSON body.

```sema
(http/not-found {:error "User not found"})
; => {:body "{\"error\":\"User not found\"}" :headers {"content-type" "application/json"} :status 404}

(define (handle-user req)
  (let ((user (find-user (:id (:params req)))))
    (if user
        (http/ok user)
        (http/not-found {:error "User not found" :id (:id (:params req))}))))
```
