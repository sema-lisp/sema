---
name: "vector-store/search"
module: "vector-store"
params: [{ name: name, type: string }, { name: query, type: bytevector }, { name: k, type: int }]
returns: "list"
---

Return the top `k` documents in the named store ranked by cosine similarity to the query embedding. Each result is a map with `:id`, `:score`, and `:metadata`.

```sema
(vector-store/search "docs" (llm/embed "greeting") 5)
```
