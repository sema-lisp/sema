---
name: "foldl"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: f, type: function, doc: "called as (f acc elem)" }, { name: init, type: any }, { name: list, type: list }]
returns: "any"
---

Left fold. `(foldl f init list)` accumulates from left to right, calling `(f acc elem)` for each
element.

```sema
(foldl + 0 '(1 2 3 4 5))                      ; => 15
(foldl (fn (acc x) (cons x acc)) '() '(1 2 3)) ; => (3 2 1)
```
