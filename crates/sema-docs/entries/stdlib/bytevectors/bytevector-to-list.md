---
name: "bytevector/to-list"
module: "bytevectors"
section: "List Conversion"
params: [{ name: bv, type: bytevector }]
returns: "list"
see_also: ["bytevector->list", "bytevector/from-list", "list/to-bytevector"]
---

Convert a bytevector to a list of integers.

```sema
(bytevector/to-list #u8(65 66))   ; => (65 66)
```
