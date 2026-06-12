---
name: "bytevector/from-list"
module: "bytevectors"
params: [{ name: bytes, type: list }]
returns: "bytevector"
---

Namespaced alias for `list->bytevector`. Convert a list of byte values into a bytevector. Each element must be an int in the range 0..255.

```sema
(bytevector/from-list '(1 2 3))   ; => #u8(1 2 3)
```
