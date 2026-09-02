---
name: "foldr"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: f, type: function, doc: "called as (f elem acc)" }, { name: init, type: any }, { name: list, type: list }]
returns: "any"
see_also: ["foldl", "fold", "reduce"]
---

Right fold: reduce a list by threading an accumulator from right to left. `(foldr f init list)` calls `(f elem acc)` on the last element first (with `acc` = `init`), then works back toward the head.

Mind the differences from `foldl`: here the **element comes first**, the accumulator second (`(f elem acc)`), and processing starts at the tail. This makes `foldr` the natural choice for rebuilding a list in original order — `(foldr cons '() xs)` reconstructs `xs` — and for any computation whose shape nests from the right.

```sema
(foldr cons '() '(1 2 3))                          ; => (1 2 3)
(foldr + 0 '(1 2 3 4 5))                           ; => 15

;; A map-like build that preserves order (foldl with cons would reverse it):
(foldr (fn (x acc) (cons (* x 10) acc)) '() '(1 2 3))  ; => (10 20 30)

;; Right-association is visible with a non-commutative op: 1-(2-(3-0))
(foldr - 0 '(1 2 3))                               ; => 2
```

See also `foldl` (left-to-right, `(f acc elem)`, tail-recursive and stack-safe). Prefer `foldl` for large lists; `foldr` recurses to the end before combining, so very long lists can grow the stack.
