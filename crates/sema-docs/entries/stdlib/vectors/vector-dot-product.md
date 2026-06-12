---
name: "vector/dot-product"
module: "vectors"
params: [{ name: a, type: bytevector }, { name: b, type: bytevector }]
returns: "float"
---

Compute the dot product of two embedding vectors. Each argument is a bytevector of packed little-endian f64 values (as produced by `embedding/list->embedding`). Both vectors must have the same, non-empty length that is a multiple of 8 bytes.

```sema
(vector/dot-product
  (embedding/list->embedding '(1.0 2.0 3.0))
  (embedding/list->embedding '(4.0 5.0 6.0)))   ; => 32.0
```
