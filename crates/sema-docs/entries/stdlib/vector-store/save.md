---
name: "vector-store/save"
module: "vector-store"
params: [{ name: name, type: string }, { name: path, type: string }]
returns: "string"
---

Persist the named vector store to disk as JSON, writing atomically. The path is optional if the store already has an associated path (e.g. from `vector-store/open`); otherwise it must be supplied. Returns the path written.

```sema
(vector-store/save "docs" "docs.json")
```
