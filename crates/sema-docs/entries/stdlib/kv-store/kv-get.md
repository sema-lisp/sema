---
name: "kv/get"
module: "kv-store"
section: "Functions"
params: [{ name: ns, type: string }, { name: key, type: string }]
returns: "any"
---

Get a value by key. Returns `nil` if the key doesn't exist.

```sema
(kv/get "config" "api-key")  ; => "sk-..." or nil
```
