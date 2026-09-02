---
name: "i64-array/make"
module: "typed-arrays"
section: "Construction"
params: [{ name: n, type: int }, { name: fill, type: int, doc: "optional; defaults to 0" }]
returns: "i64-array"
see_also: ["f64-array/make", "i64-array", "i64-array/range"]
---

Create an i64 array of a given length, optionally filled with a value (default `0`).

```sema
(i64-array/make 5)     ; => #i64(0 0 0 0 0)
(i64-array/make 3 42)  ; => #i64(42 42 42)
```
