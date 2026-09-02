---
name: "mutable-array/->vector"
module: "mutable"
section: "Mutable Containers"
params: [{ name: arr, type: mutable-array }]
returns: "vector"
see_also: ["mutable-array/new", "vector", "vector->list"]
---

Freeze a mutable array into an immutable vector — a snapshot copy: later mutation of the array does not change the returned vector. This is the hand-off point from an imperative accumulation loop back to the persistent world (sortable, printable, usable as map values).

```sema
(define a (mutable-array/new))
(mutable-array/push! a 1)
(define v (mutable-array/->vector a))
(mutable-array/set! a 0 9)
v   ; => [1]   ; the snapshot is unaffected
```
