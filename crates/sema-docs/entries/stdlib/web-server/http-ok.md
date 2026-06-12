---
name: "http/ok"
module: "web-server"
section: "Response Helpers"
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
