---
name: "bytevector/set!"
module: "bytevectors"
section: "Access & Mutation"
---

Set the byte at a given index. Uses copy-on-write — the original bytevector is unchanged.

```sema
(bytevector/set! #u8(1 2 3) 0 9)   ; => #u8(9 2 3)
```
