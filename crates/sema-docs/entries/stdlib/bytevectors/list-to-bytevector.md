---
name: "list/to-bytevector"
module: "bytevectors"
section: "List Conversion"
params: [{ name: bytes, type: list }]
returns: "bytevector"
see_also: ["list->bytevector", "bytevector/from-list", "bytevector/to-list"]
---

Convert a list of integers to a bytevector.

```sema
(list/to-bytevector '(1 2 3))   ; => #u8(1 2 3)
```
