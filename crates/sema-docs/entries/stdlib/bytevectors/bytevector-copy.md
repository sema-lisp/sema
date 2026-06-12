---
name: "bytevector/copy"
module: "bytevectors"
section: "Copy & Append"
---

Copy a slice of a bytevector. `(bytevector/copy bv start end)`.

```sema
(bytevector/copy #u8(1 2 3 4 5) 1 3)   ; => #u8(2 3)
```
