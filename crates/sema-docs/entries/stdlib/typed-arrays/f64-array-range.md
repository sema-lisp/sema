---
name: "f64-array/range"
module: "typed-arrays"
section: "Construction"
params: [{ name: start, type: number }, { name: end, type: number }, { name: step, type: number, doc: "optional; defaults to 1" }]
returns: "f64-array"
see_also: ["i64-array/range", "f64-array/make", "range"]
---

Create an f64 array from a numeric range. `(f64-array/range start end)` or `(f64-array/range start end step)`.

Arguments must be finite, and the step must be nonzero and change each value
at f64 precision. The result is limited to 8,388,608 elements (64 MiB).
Non-finite arguments and oversized length estimates fail before allocation.
Progress and the element limit are also checked while constructing the range.

```sema
(f64-array/range 0 5)        ; => #f64(0 1 2 3 4)
(f64-array/range 0 1 0.25)   ; => #f64(0 0.25 0.5 0.75)
```
