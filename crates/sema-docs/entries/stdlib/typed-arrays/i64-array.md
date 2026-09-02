---
name: "i64-array"
module: "typed-arrays"
section: "Construction"
syntax: "(i64-array value ...)"
returns: "i64-array"
see_also: ["f64-array", "i64-array/make", "i64-array/from-list", "f64-array?"]
---

Create an i64 array from values.

```sema
(i64-array 1 2 3)  ; => #i64(1 2 3)
(i64-array)        ; => #i64()
```
