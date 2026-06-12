---
name: "bytevector/length"
module: "bytevectors"
section: "Access & Mutation"
---

Return the length of a bytevector.

```sema
(bytevector/length #u8(1 2 3))   ; => 3
(bytevector/length #u8())        ; => 0
```
