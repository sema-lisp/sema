---
name: "list/reject"
module: "lists"
section: "Filtering"
params: [{ name: pred, type: function }, { name: seq, type: list }]
returns: "list"
see_also: ["filter", "partition", "list/find"]
---

Return elements that do NOT satisfy a predicate (inverse of `filter`).

```sema
(list/reject even? '(1 2 3 4 5))   ; => (1 3 5)
```
