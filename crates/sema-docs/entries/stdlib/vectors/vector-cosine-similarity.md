---
name: "vector/cosine-similarity"
module: "vectors"
params: [{ name: a, type: bytevector }, { name: b, type: bytevector }]
returns: "float"
---

Compute the cosine similarity between two embedding vectors (dot product divided by the product of magnitudes). Each argument is a bytevector of packed little-endian f64 values (as produced by `embedding/list->embedding`). Both vectors must have the same, non-empty length that is a multiple of 8 bytes. Returns `0.0` if either vector has zero magnitude.

```sema
(vector/cosine-similarity
  (embedding/list->embedding '(1.0 0.0))
  (embedding/list->embedding '(1.0 0.0)))   ; => 1.0
```
