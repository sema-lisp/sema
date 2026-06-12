---
name: "context/get"
module: "context"
section: "Core Functions"
---

Retrieve a value by key. Returns `nil` if the key doesn't exist.

```sema
(context/get :trace-id)   ; => "abc-123"
(context/get :missing)    ; => nil
```
