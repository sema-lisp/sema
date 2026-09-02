---
name: "filter"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: pred, type: function }, { name: seq, type: list }]
returns: "list"
see_also: ["list/reject", "map", "partition", "list/find"]
---

Return a new list containing only the elements that **satisfy** `pred` (those for which it returns truthy); order is preserved and the input is untouched.

```sema
(filter even? '(1 2 3 4 5))            ; => (2 4)
(filter string? '(1 "a" 2))            ; => ("a")
(filter (fn (x) (> x 0)) '(-2 3 -1 5)) ; => (3 5)
(filter even? '(1 3 5))                ; => ()
```

See also `list/reject` (keeps the elements that *fail* the predicate) and `map` (transforms every element rather than selecting). To both filter and transform, chain `filter` then `map`, or use `flat-map`.

