---
name: "f64-array/map"
module: "typed-arrays"
section: "Higher-Order Functions"
aliases: ["i64-array/map"]
---

Apply a function to each element, returning a new typed array. The callback must return the matching numeric type.

```sema
(f64-array/map (lambda (x) (* x 2.0)) (f64-array 1.0 2.0 3.0))
; => #f64(2 4 6)

(i64-array/map (lambda (x) (* x x)) (i64-array 1 2 3 4))
; => #i64(1 4 9 16)
```
