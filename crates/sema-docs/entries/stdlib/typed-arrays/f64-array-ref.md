---
name: "f64-array/ref"
module: "typed-arrays"
section: "Access & Mutation"
aliases: ["i64-array/ref"]
---

Get the element at a given index.

```sema
(f64-array/ref (f64-array 1.0 2.0 3.0) 1)  ; => 2.0
(i64-array/ref (i64-array 10 20 30) 0)      ; => 10
```
