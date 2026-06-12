---
name: "bytevector/new"
module: "bytevectors"
section: "Construction"
---

Create a bytevector of a given length, optionally filled with a value.

```sema
(bytevector/new 4)       ; => #u8(0 0 0 0)
(bytevector/new 3 255)   ; => #u8(255 255 255)
```
