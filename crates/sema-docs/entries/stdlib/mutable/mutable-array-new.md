---
name: "mutable-array/new"
module: "mutable"
section: "Mutable Containers"
params: [{ name: cap-or-n, type: int, doc: "capacity hint (1-arg) or element count (2-arg)", optional: true }, { name: fill, type: any, doc: "fill value for the 2-arg form", optional: true }]
returns: "mutable-array"
see_also: ["mutable-array/push!", "mutable-array/get", "mutable-array/->vector", "mutable-cell/new"]
---

Create a mutable array — an in-place mutable container for imperative hot loops, unlike the persistent (copy-on-write) `vector`. With no arguments it is empty; with one argument the array is still empty but pre-allocates capacity for that many pushes; with two arguments it holds `n` copies of `fill`, ready for indexed `mutable-array/set!`.

Mutable arrays are shared by reference: mutating through one binding is visible through every other binding to the same array. `equal?` compares contents; use `mutable-array/->vector` to freeze a snapshot for the immutable world. Mutable arrays cannot be used as map keys.

```sema
(define a (mutable-array/new))        ; empty
(mutable-array/push! a 1)
(mutable-array/->vector a)            ; => [1]

(mutable-array/->vector (mutable-array/new 3 0))  ; => [0 0 0]
(mutable-array/length (mutable-array/new 64))     ; => 0 (capacity only)
```
