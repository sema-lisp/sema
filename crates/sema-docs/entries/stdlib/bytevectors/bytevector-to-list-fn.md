---
name: "bytevector->list"
module: "bytevectors"
params: [{ name: bv, type: bytevector }]
returns: "list"
---

Convert a bytevector into a list of byte values (each an int in 0..255). Legacy Scheme name; `bytevector/to-list` is the namespaced alias.

```sema
(bytevector->list #u8(65 66))   ; => (65 66)
```
