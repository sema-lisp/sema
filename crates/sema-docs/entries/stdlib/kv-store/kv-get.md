---
name: "kv/get"
module: "kv-store"
section: "Functions"
params: [{ name: ns, type: string }, { name: key, type: string }]
returns: "any"
see_also: ["kv/set", "kv/delete", "kv/keys"]
---

Get a value by key. Returns `nil` if the key doesn't exist.

This reads an in-memory field with no I/O, so it never offloads or queues — if the store has a `kv/set`/`kv/delete` flush in flight (e.g. a concurrent `async/spawn` call), it fails immediately with a busy error rather than waiting for it.

```sema
(kv/get "config" "api-key")  ; => "sk-..." or nil
```
