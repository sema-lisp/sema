---
name: "http/ok"
module: "web-server"
section: "Response Helpers"
params: [{ name: body, type: any, doc: "value JSON-encoded into the response body" }]
returns: "map"
see_also: ["http/created", "http/error", "http/html", "http/text"]
---

Return 200 with a JSON-encoded body.

```sema
(pprint (http/ok {:message "success"}))
; => {:body "{"message":"success"}"
;     :headers {"content-type" "application/json"}
;     :status 200}

(pprint (http/ok [1 2 3]))
; => {:body "[1,2,3]" :headers {"content-type" "application/json"} :status 200}
```
