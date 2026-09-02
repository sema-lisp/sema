---
name: "list/chunk"
module: "lists"
section: "Grouping"
params: [{ name: n, type: int }, { name: seq, type: list }]
returns: "list"
see_also: ["list/sliding", "list/split-at", "partition", "list/page"]
---

Split a list into chunks of a given size.

```sema
(list/chunk 2 '(1 2 3 4 5))   ; => ((1 2) (3 4) (5))
(list/chunk 3 '(1 2 3 4 5 6)) ; => ((1 2 3) (4 5 6))
```
