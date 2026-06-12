---
name: "vector-store/count"
module: "vector-store"
params: [{ name: name, type: string }]
returns: "int"
---

Return the number of documents in the named vector store.

```sema
(vector-store/count "docs")   ; => 42
```
