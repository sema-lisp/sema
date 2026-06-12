---
name: "kv/delete"
module: "kv-store"
section: "Functions"
---

Delete a key. Returns `#t` if the key existed, `#f` otherwise. Flushes to disk immediately.

```sema
(kv/delete "config" "api-key")  ; => #t
(kv/delete "config" "api-key")  ; => #f (already deleted)
```
