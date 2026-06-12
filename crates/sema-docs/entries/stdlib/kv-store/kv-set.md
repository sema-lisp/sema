---
name: "kv/set"
module: "kv-store"
section: "Functions"
---

Set a key-value pair. The value is serialized as JSON. Returns the value. Flushes to disk immediately.

```sema
(kv/set "config" "api-key" "sk-...")
(kv/set "config" "retries" 3)
(kv/set "config" "tags" '("a" "b" "c"))
(kv/set "config" "user" {:name "Alice" :role "admin"})
```
