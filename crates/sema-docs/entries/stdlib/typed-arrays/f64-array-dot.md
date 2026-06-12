---
name: "f64-array/dot"
module: "typed-arrays"
section: "Aggregation"
---

Compute the dot product of two f64 arrays (must be the same length).

```sema
(f64-array/dot (f64-array 1.0 2.0 3.0) (f64-array 4.0 5.0 6.0))
; => 32.0  (1*4 + 2*5 + 3*6)
```
