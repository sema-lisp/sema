---
name: "kv/keys"
module: "kv-store"
section: "Functions"
params: [{ name: ns, type: string }]
returns: "list"
see_also: ["kv/get", "kv/set", "kv/delete"]
---

List all keys in the store. Returns a list of strings.

This reads an in-memory field with no I/O, so it never offloads or queues — if the store has a `kv/set`/`kv/delete` flush in flight (e.g. a concurrent `async/spawn` call), it fails immediately with a busy error rather than waiting for it.

```sema
(kv/keys "config")  ; => ("api-key" "retries" "tags")
```
