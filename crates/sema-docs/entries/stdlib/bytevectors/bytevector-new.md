---
name: "bytevector/new"
module: "bytevectors"
section: "Construction"
params: [{ name: n, type: int }, { name: fill, type: int, doc: "optional; defaults to 0" }]
returns: "bytevector"
see_also: ["make-bytevector", "bytevector/make", "bytevector"]
---

Create a bytevector of a given length, optionally filled with a value.

```sema
(bytevector/new 4)       ; => #u8(0 0 0 0)
(bytevector/new 3 255)   ; => #u8(255 255 255)
```
