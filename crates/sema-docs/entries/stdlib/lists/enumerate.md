---
name: "enumerate"
module: "lists"
section: "Higher-Order Functions"
syntax: "(enumerate seq)"
returns: "list"
---

Pair each element of `seq` with its 0-based index, returning a list of `(index element)` lists. Accepts a list or vector; the input is never mutated and the result is always a list.

```sema
(enumerate '(10 20 30))   ; => ((0 10) (1 20) (2 30))
(enumerate (vector 'a 'b)) ; => ((0 a) (1 b))
```

`(enumerate xs)` is equivalent to `(map-indexed list xs)`. See also: `map-indexed` (transform with the index instead of just pairing).
