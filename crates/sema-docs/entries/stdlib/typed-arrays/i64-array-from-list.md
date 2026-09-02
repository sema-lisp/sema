---
name: "i64-array/from-list"
module: "typed-arrays"
section: "Construction"
params: [{ name: seq, type: list }]
returns: "i64-array"
see_also: ["f64-array/from-list", "i64-array", "i64-array/make"]
---

Convert a list of integers to an i64 array.

```sema
(i64-array/from-list '(10 20 30))  ; => #i64(10 20 30)
```
