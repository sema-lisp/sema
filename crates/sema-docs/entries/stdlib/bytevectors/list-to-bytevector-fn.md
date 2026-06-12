---
name: "list->bytevector"
module: "bytevectors"
params: [{ name: bytes, type: list }]
returns: "bytevector"
---

Convert a list of byte values into a bytevector. Each element must be an int in the range 0..255. Legacy Scheme name; `bytevector/from-list` and `list/to-bytevector` are the namespaced aliases.

```sema
(list->bytevector '(1 2 3))   ; => #u8(1 2 3)
```
