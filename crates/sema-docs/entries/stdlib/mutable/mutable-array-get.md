---
name: "mutable-array/get"
module: "mutable"
section: "Mutable Containers"
params: [{ name: arr, type: mutable-array }, { name: index, type: int, doc: "zero-based" }, { name: default, type: any, doc: "returned when the index is out of bounds", optional: true }]
returns: "any"
see_also: ["mutable-array/set!", "mutable-array/length", "mutable-array/new"]
---

Read the element at a zero-based index of a mutable array. Out of bounds is an error unless a `default` is supplied. `(nth arr i)` also works on mutable arrays.

```sema
(define a (mutable-array/new 2 :x))
(mutable-array/get a 1)            ; => :x
(mutable-array/get a 9 :missing)   ; => :missing
```
