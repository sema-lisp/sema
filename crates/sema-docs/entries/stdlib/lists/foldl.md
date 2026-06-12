---
name: "foldl"
module: "lists"
section: "Higher-Order Functions"
---

Left fold. `(foldl f init list)` accumulates from left to right, calling `(f acc elem)` for each
element.

```sema
(foldl + 0 '(1 2 3 4 5))                      ; => 15
(foldl (fn (acc x) (cons x acc)) '() '(1 2 3)) ; => (3 2 1)
```
