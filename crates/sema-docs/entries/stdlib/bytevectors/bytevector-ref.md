---
name: "bytevector/ref"
module: "bytevectors"
section: "Access & Mutation"
params: [{ name: bv, type: bytevector }, { name: index, type: int }]
returns: "int"
---

Return the byte at a given index.

```sema
(bytevector/ref #u8(10 20 30) 1)   ; => 20
(bytevector/ref #u8(10 20 30) 0)   ; => 10
```
