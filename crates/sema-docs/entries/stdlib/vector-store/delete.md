---
name: "vector-store/delete"
module: "vector-store"
params: [{ name: name, type: string }, { name: id, type: string }]
returns: "bool"
---

Remove the document with the given id from the named store. Returns true if a document was removed, false otherwise.

```sema
(vector-store/delete "docs" "doc-1")
```
