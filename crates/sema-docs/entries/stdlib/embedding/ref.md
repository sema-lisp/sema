---
name: "embedding/ref"
module: "embedding"
params: [{ name: embedding, type: bytevector }, { name: index, type: int }]
returns: "float"
---

Return the embedding value at the given dimension index as a float. Errors if the index is out of bounds.

```sema
(embedding/ref emb 0)   ; => 0.0123
```
