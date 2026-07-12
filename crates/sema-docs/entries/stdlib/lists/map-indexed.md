---
name: "map-indexed"
module: "lists"
section: "Higher-Order Functions"
syntax: "(map-indexed f seq)"
returns: "list"
---

Apply `f` to each element of `seq` along with its 0-based index, collecting the results into a new list. `f` is called as `(f index element)`. Accepts a list or vector; the input is never mutated and the result is always a list.

```sema
(map-indexed (fn (i x) (list i x)) '(10 20 30))   ; => ((0 10) (1 20) (2 30))
(map-indexed (fn (i x) (+ i x)) (vector 10 20 30)) ; => (10 21 32)
```

See also: `map` (no index), `enumerate` (pairs elements with their index without transforming them).
