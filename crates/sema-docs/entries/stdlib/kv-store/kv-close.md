---
name: "kv/close"
module: "kv-store"
section: "Functions"
params: [{ name: ns, type: string }]
returns: "nil"
see_also: ["kv/open", "kv/set"]
---

Close a store, flushing data and freeing the handle. Returns `nil`.

```sema
(kv/close "config")
```

Data is safe even without calling `kv/close` (every write already flushes), but closing frees memory and releases the store name.

Closing a store with a `kv/set`/`kv/delete` flush in flight (a concurrent `async/spawn` call) fails with a busy error instead of racing the write — wait for it to resolve first. Closing a tombstoned store (one whose flush was cancelled mid-flight) is a no-op that frees the handle, so the same name can be `kv/open`ed again.
