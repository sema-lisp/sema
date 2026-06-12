---
name: "bytevector/u8-ref"
module: "bytevectors"
params: [{ name: bv, type: bytevector }, { name: index, type: int }]
returns: "int"
---

Namespaced alias for `bytevector-u8-ref`. Return the byte (0..255) at `index` in a bytevector. Signals an error if `index` is out of range.

```sema
(bytevector/u8-ref #u8(10 20 30) 1)   ; => 20
```
