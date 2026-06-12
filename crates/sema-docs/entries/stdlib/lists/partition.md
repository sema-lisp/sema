---
name: "partition"
module: "lists"
section: "Sublists"
---

Split a list into two lists based on a predicate. Returns a list of two lists: elements that satisfy the predicate and those that don't.

```sema
(partition even? '(1 2 3 4 5))   ; => ((2 4) (1 3 5))
```
