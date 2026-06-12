---
name: "vector-store/create"
module: "vector-store"
params: [{ name: name, type: string }]
returns: "string"
---

Create a new, empty in-memory vector store registered under the given name, returning the name. Use `vector-store/open` instead to load from or persist to disk.

```sema
(vector-store/create "docs")
```
