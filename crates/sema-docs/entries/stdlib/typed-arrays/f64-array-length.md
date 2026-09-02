---
name: "f64-array/length"
module: "typed-arrays"
section: "Access & Mutation"
aliases: ["i64-array/length"]
params: [{ name: arr, type: "f64-array | i64-array" }]
returns: "int"
see_also: ["f64-array/ref", "f64-array/make", "length"]
---

Return the number of elements.

```sema
(f64-array/length (f64-array 1.0 2.0 3.0))  ; => 3
(i64-array/length (i64-array/make 10))       ; => 10
```
