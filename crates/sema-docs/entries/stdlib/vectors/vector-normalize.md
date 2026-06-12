---
name: "vector/normalize"
module: "vectors"
params: [{ name: v, type: bytevector }]
returns: "bytevector"
---

Return a unit-length copy of an embedding vector (each component divided by the vector's magnitude). The argument is a bytevector of packed little-endian f64 values (as produced by `embedding/list->embedding`), non-empty with a length that is a multiple of 8 bytes. A zero vector is returned unchanged (all zeros).

```sema
;; (3.0 4.0) has magnitude 5, so it normalizes to (0.6 0.8)
(embedding/->list (vector/normalize (embedding/list->embedding '(3.0 4.0))))
; => (0.6 0.8)
```
