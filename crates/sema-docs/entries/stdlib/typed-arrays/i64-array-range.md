---
name: "i64-array/range"
module: "typed-arrays"
section: "Construction"
params: [{ name: start, type: int }, { name: end, type: int }, { name: step, type: int, doc: "optional; defaults to 1" }]
returns: "i64-array"
see_also: ["f64-array/range", "i64-array/make", "range"]
---

Create an i64 array from an integer range.

```sema
(i64-array/range 0 5)      ; => #i64(0 1 2 3 4)
(i64-array/range 0 10 2)   ; => #i64(0 2 4 6 8)
```
