---
name: "kv/delete"
module: "kv-store"
section: "Functions"
params: [{ name: ns, type: string }, { name: key, type: string }]
returns: "bool"
see_also: ["kv/set", "kv/get", "kv/keys"]
---

Delete a key. Returns `#t` if the key existed, `#f` otherwise. Flushes to disk immediately.

Concurrent calls against the same store serialize: inside `async/spawn`, a call queues automatically behind any other `kv/set`/`kv/delete` flush already in flight on that store instead of racing the write-through. If a queued flush is cancelled before it completes, the store handle is tombstoned — every later `kv/*` call on it fails until `kv/close` frees the handle and it's reopened with `kv/open`.

```sema
(kv/delete "config" "api-key")  ; => #t
(kv/delete "config" "api-key")  ; => #f (already deleted)
```
