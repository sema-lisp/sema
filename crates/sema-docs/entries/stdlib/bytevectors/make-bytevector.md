---
name: "make-bytevector"
module: "bytevectors"
params: [{ name: size, type: int }, { name: fill, type: int, optional: true }]
returns: "bytevector"
---

Create a bytevector of `size` bytes, each initialized to `fill` (default `0`). `size` must be non-negative and `fill` must be in the range 0..255.

```sema
(make-bytevector 4)       ; => #u8(0 0 0 0)
(make-bytevector 3 255)   ; => #u8(255 255 255)
```
