---
name: "mutable-array/push!"
module: "mutable"
section: "Mutable Containers"
params: [{ name: arr, type: mutable-array }, { name: value, type: any }]
returns: "mutable-array"
see_also: ["mutable-array/new", "mutable-array/set!", "mutable-array/length"]
---

Append a value to the end of a mutable array, in place. Returns the array itself, so pushes chain and work as the accumulator of a fold.

```sema
(define a (mutable-array/new))
(mutable-array/push! (mutable-array/push! a 1) 2)
(mutable-array/->vector a)   ; => [1 2]

;; As a fold accumulator:
(mutable-array/->vector
  (foldl (fn (acc x) (mutable-array/push! acc (* x x)))
         (mutable-array/new)
         '(1 2 3)))
; => [1 4 9]
```
