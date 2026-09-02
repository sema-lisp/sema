---
name: "map"
module: "lists"
section: "Higher-Order Functions"
syntax: "(map f lst ...)"
returns: "list"
see_also: ["filter", "foldl", "for-each", "map-indexed", "flat-map"]
---

Apply a function to each element of a list, collecting the results into a new list. The input is never mutated.

Given several lists, `map` walks them in lockstep, calling the function with one element from each. Iteration stops at the **shortest** list, so mismatched lengths are truncated rather than erroring.

```sema
(map (fn (x) (* x x)) '(1 2 3))   ; => (1 4 9)
(map + '(1 2 3) '(10 20 30))      ; => (11 22 33)
(map + '(1 2 3) '(10 20))         ; => (11 22)   ; stops at shortest
```

See also: `mapcar` (alias), `for-each` (run side effects, discard results), `filter`/`list/reject` (keep/drop by predicate).
