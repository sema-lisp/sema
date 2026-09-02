---
name: "bytevector/append"
module: "bytevectors"
section: "Copy & Append"
syntax: "(bytevector/append bv ...)"
returns: "bytevector"
see_also: ["bytevector-append", "bytevector/copy", "bytevector"]
---

Concatenate bytevectors.

```sema
(bytevector/append #u8(1 2) #u8(3 4))   ; => #u8(1 2 3 4)
```
