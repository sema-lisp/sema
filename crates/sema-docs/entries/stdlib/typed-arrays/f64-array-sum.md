---
name: "f64-array/sum"
module: "typed-arrays"
section: "Aggregation"
aliases: ["i64-array/sum"]
params: [{ name: arr, type: "f64-array | i64-array" }]
returns: "number"
see_also: ["f64-array/dot", "f64-array/fold", "list/sum"]
---

Sum all elements. Runs in a tight Rust loop with no boxing overhead.

```sema
(f64-array/sum (f64-array 1.0 2.0 3.0))  ; => 6.0
(i64-array/sum (i64-array 1 2 3 4 5))    ; => 15
```
