---
name: "f64-array"
module: "typed-arrays"
section: "Construction"
syntax: "(f64-array value ...)"
returns: "f64-array"
see_also: ["i64-array", "f64-array/make", "f64-array/from-list", "f64-array?"]
---

Create an f64 array from values.

```sema
(f64-array 1.0 2.5 3.7)  ; => #f64(1 2.5 3.7)
(f64-array)               ; => #f64()
```
