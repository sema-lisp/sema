---
name: "mutable-array/set!"
module: "mutable"
section: "Mutable Containers"
params: [{ name: arr, type: mutable-array }, { name: index, type: int, doc: "zero-based; must be < length" }, { name: value, type: any }]
returns: "mutable-array"
see_also: ["mutable-array/get", "mutable-array/push!", "mutable-array/new"]
---

Overwrite the element at a zero-based index of a mutable array, in place. The slot must already exist (`index < length`) — use `mutable-array/push!` to grow. Returns the array. Unlike `vector` updates, no copy is made: every binding to the array sees the new value.

```sema
(define stats (mutable-array/new 4 0))   ; [min max sum count] accumulator
(mutable-array/set! stats 2 (+ (mutable-array/get stats 2) 57))
(mutable-array/->vector stats)   ; => [0 0 57 0]
```
