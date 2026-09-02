---
name: "kv/set"
module: "kv-store"
section: "Functions"
params: [{ name: ns, type: string }, { name: key, type: string }, { name: value, type: any }]
returns: "any"
see_also: ["kv/get", "kv/delete", "kv/keys"]
---

Set a key-value pair. The value is serialized as JSON. Returns the value. Flushes to disk immediately.

Concurrent calls against the same store serialize: inside `async/spawn`, a call queues automatically behind any other `kv/set`/`kv/delete` flush already in flight on that store instead of racing the write-through. If a queued flush is cancelled before it completes, the store handle is tombstoned — every later `kv/*` call on it fails until `kv/close` frees the handle and it's reopened with `kv/open`.

```sema
(kv/set "config" "api-key" "sk-...")
(kv/set "config" "retries" 3)
(kv/set "config" "tags" '("a" "b" "c"))
(kv/set "config" "user" {:name "Alice" :role "admin"})
```
