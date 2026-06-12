---
name: "embedding/list->embedding"
module: "embedding"
params: [{ name: nums }]
returns: "bytevector"
---

Convert a list or vector of numbers into an embedding bytevector (little-endian f64 per element). Inverse of `embedding/->list`.

```sema
(embedding/list->embedding [0.1 0.2 0.3])
```
