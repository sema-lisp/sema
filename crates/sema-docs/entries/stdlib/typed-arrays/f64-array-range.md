---
name: "f64-array/range"
module: "typed-arrays"
section: "Construction"
---

Create an f64 array from a numeric range. `(f64-array/range start end)` or `(f64-array/range start end step)`.

```sema
(f64-array/range 0 5)        ; => #f64(0 1 2 3 4)
(f64-array/range 0 1 0.25)   ; => #f64(0 0.25 0.5 0.75)
```
