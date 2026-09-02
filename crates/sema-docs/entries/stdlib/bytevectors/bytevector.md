---
name: "bytevector"
module: "bytevectors"
section: "Construction"
syntax: "(bytevector byte ...)"
returns: "bytevector"
see_also: ["make-bytevector", "bytevector/from-list", "bytevector/append"]
---

Create a bytevector from byte values.

```sema
(bytevector 1 2 3)       ; => #u8(1 2 3)
(bytevector)             ; => #u8()
```
