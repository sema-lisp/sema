---
name: "kv/get"
module: "kv-store"
section: "Functions"
---

Get a value by key. Returns `nil` if the key doesn't exist.

```sema
(kv/get "config" "api-key")  ; => "sk-..." or nil
```
