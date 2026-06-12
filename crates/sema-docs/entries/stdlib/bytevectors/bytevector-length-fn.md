---
name: "bytevector-length"
module: "bytevectors"
params: [{ name: bv, type: bytevector }]
returns: "int"
---

Return the number of bytes in a bytevector. Legacy Scheme name; `bytevector/length` is the namespaced alias.

```sema
(bytevector-length #u8(1 2 3))   ; => 3
```
