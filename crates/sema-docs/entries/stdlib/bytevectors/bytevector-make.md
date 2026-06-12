---
name: "bytevector/make"
module: "bytevectors"
params: [{ name: size, type: int }, { name: fill, type: int, optional: true }]
returns: "bytevector"
---

Namespaced alias for `make-bytevector`. Create a bytevector of `size` bytes, each initialized to `fill` (default `0`). `size` must be non-negative and `fill` must be in the range 0..255.

```sema
(bytevector/make 4)       ; => #u8(0 0 0 0)
(bytevector/make 3 255)   ; => #u8(255 255 255)
```
