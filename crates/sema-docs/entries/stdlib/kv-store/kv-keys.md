---
name: "kv/keys"
module: "kv-store"
section: "Functions"
---

List all keys in the store. Returns a list of strings.

```sema
(kv/keys "config")  ; => ("api-key" "retries" "tags")
```
