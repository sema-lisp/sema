---
name: "sort"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: lst, type: list }, { name: cmp, type: function, doc: "optional comparator" }]
returns: "list"
---

Sort a list in ascending order.

```sema
(sort '(3 1 4 1 5))   ; => (1 1 3 4 5)
```
