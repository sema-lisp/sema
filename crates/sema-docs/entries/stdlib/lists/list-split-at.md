---
name: "list/split-at"
module: "lists"
section: "Splitting"
params: [{ name: seq, type: list }, { name: index, type: int }]
returns: "list"
see_also: ["list/chunk", "take", "drop", "partition"]
---

Split a list at a given index, returning two lists.

```sema
(list/split-at '(1 2 3 4 5) 3)   ; => ((1 2 3) (4 5))
```
