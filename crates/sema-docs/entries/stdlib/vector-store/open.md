---
name: "vector-store/open"
module: "vector-store"
params: [{ name: name, type: string }, { name: path, type: string }]
returns: "string"
---

Open a vector store under the given name, loading it from the JSON file at path if it exists or creating an empty one otherwise, and associate the path so later `vector-store/save` calls can omit it. Returns the name.

```sema
(vector-store/open "docs" "docs.json")
```
