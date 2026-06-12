---
name: "f64-array/fold"
module: "typed-arrays"
section: "Higher-Order Functions"
aliases: ["i64-array/fold"]
---

Fold over a typed array with an accumulator.

```sema
(f64-array/fold (lambda (acc x) (+ acc x)) 0.0 (f64-array 1.0 2.0 3.0))
; => 6.0

(i64-array/fold (lambda (acc x) (max acc x)) 0 (i64-array 3 1 4 1 5))
; => 5
```
