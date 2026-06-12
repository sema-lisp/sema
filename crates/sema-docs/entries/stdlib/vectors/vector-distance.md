---
name: "vector/distance"
module: "vectors"
params: [{ name: a, type: bytevector }, { name: b, type: bytevector }]
returns: "float"
---

Compute the Euclidean (L2) distance between two embedding vectors. Each argument is a bytevector of packed little-endian f64 values (as produced by `embedding/list->embedding`). Both vectors must have the same, non-empty length that is a multiple of 8 bytes.

```sema
(vector/distance
  (embedding/list->embedding '(0.0 0.0))
  (embedding/list->embedding '(3.0 4.0)))   ; => 5.0
```
