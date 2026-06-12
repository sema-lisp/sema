---
name: "f64-array/set!"
module: "typed-arrays"
section: "Access & Mutation"
aliases: ["i64-array/set!"]
---

Set the element at a given index. Uses copy-on-write -- the original array is unchanged unless it has a single reference.

```sema
(f64-array/set! (f64-array 1.0 2.0 3.0) 1 9.9)  ; => #f64(1 9.9 3)
(i64-array/set! (i64-array 10 20 30) 2 99)       ; => #i64(10 20 99)
```
