---
name: "f64-array/from-list"
module: "typed-arrays"
section: "Construction"
params: [{ name: seq, type: list }]
returns: "f64-array"
see_also: ["i64-array/from-list", "f64-array", "f64-array/make"]
---

Convert a list of numbers to an f64 array.

```sema
(f64-array/from-list '(1 2 3))  ; => #f64(1 2 3)
```
