---
name: "bytevector/u8-set!"
module: "bytevectors"
params: [{ name: bv, type: bytevector }, { name: index, type: int }, { name: byte, type: int }]
returns: "bytevector"
---

Namespaced alias for `bytevector-u8-set!`. Return a new bytevector with the byte at `index` set to `byte` (0..255). Uses copy-on-write — the original bytevector is unchanged. Signals an error if `index` is out of range or `byte` is outside 0..255.

```sema
(bytevector/u8-set! #u8(1 2 3) 0 9)   ; => #u8(9 2 3)
```
