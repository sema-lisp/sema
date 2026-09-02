---
name: "context/get"
module: "context"
section: "Core Functions"
params: [{ name: key, type: keyword }]
returns: "any or nil"
see_also: ["context/set", "context/has?", "context/all"]
---

Retrieve a value by key. Returns `nil` if the key doesn't exist.

```sema
(context/get :trace-id)   ; => "abc-123"
(context/get :missing)    ; => nil
```
