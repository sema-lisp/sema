---
name: "bytevector"
module: "bytevectors"
section: "Construction"
syntax: "(bytevector byte ...)"
returns: "bytevector"
---

Create a bytevector from byte values.

```sema
(bytevector 1 2 3)       ; => #u8(1 2 3)
(bytevector)             ; => #u8()
```
