---
name: "list/chunk"
module: "lists"
section: "Grouping"
---

Split a list into chunks of a given size.

```sema
(list/chunk 2 '(1 2 3 4 5))   ; => ((1 2) (3 4) (5))
(list/chunk 3 '(1 2 3 4 5 6)) ; => ((1 2 3) (4 5 6))
```
