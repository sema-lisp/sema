---
name: "i64-array/range"
module: "typed-arrays"
section: "Construction"
---

Create an i64 array from an integer range.

```sema
(i64-array/range 0 5)      ; => #i64(0 1 2 3 4)
(i64-array/range 0 10 2)   ; => #i64(0 2 4 6 8)
```
