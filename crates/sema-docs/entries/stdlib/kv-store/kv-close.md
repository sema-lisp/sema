---
name: "kv/close"
module: "kv-store"
section: "Functions"
---

Close a store, flushing data and freeing the handle. Returns `nil`.

```sema
(kv/close "config")
```

Data is safe even without calling `kv/close` (every write already flushes), but closing frees memory and releases the store name.
