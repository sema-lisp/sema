---
name: "bytevector-copy"
module: "bytevectors"
params: [{ name: bv, type: bytevector }, { name: start, type: int, optional: true }, { name: end, type: int, optional: true }]
returns: "bytevector"
---

Copy a bytevector, optionally restricting to the half-open range `start..end` (default `start` is `0`, `end` is the length). Signals an error if the range is out of bounds. Legacy Scheme name; `bytevector/copy` is the namespaced alias.

```sema
(bytevector-copy #u8(1 2 3 4 5) 1 4)   ; => #u8(2 3 4)
```
