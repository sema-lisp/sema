---
name: "f64-array/make"
module: "typed-arrays"
section: "Construction"
---

Create an f64 array of a given length, optionally filled with a value (default `0.0`).

```sema
(f64-array/make 5)       ; => #f64(0 0 0 0 0)
(f64-array/make 3 1.5)   ; => #f64(1.5 1.5 1.5)
```
