---
name: "foldl"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: f, type: function, doc: "called as (f acc elem)" }, { name: init, type: any }, { name: list, type: list }]
returns: "any"
see_also: ["foldr", "fold", "reduce", "map"]
---

Left fold: reduce a list to a single value by threading an accumulator from left to right. `(foldl f init list)` starts with `init` and calls `(f acc elem)` for each element, feeding each result back in as the next `acc`.

This is the workhorse behind sum, product, max, building a map, or reversing a list — any "boil a sequence down to one thing" computation. Note the argument order: the **accumulator comes first**, the element second (`(f acc elem)`). When `f` is not commutative (like `cons` or `-`), this direction matters.

```sema
(foldl + 0 '(1 2 3 4 5))                       ; => 15
(foldl max 0 '(3 1 4 1 5))                      ; => 5

;; Walking left-to-right and prepending reverses the list:
(foldl (fn (acc x) (cons x acc)) '() '(1 2 3))  ; => (3 2 1)

;; Non-commutative ops expose the direction: ((0-1)-2)-3
(foldl - 0 '(1 2 3))                            ; => -6
```

See also `foldr` (folds right-to-left, calling `(f elem acc)`) and `fold` (an alias of `foldl`). `foldl` is the tail-recursive, stack-safe direction — reach for it by default; choose `foldr` when the result is naturally built from the right, like rebuilding a list with `cons`.
