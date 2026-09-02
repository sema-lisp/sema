---
name: "list/shuffle"
module: "lists"
section: "Random"
params: [{ name: seq, type: list }]
returns: "list"
see_also: ["list/pick", "sort", "list/unique"]
---

Return a randomly shuffled copy of a list.

```sema
(list/shuffle '(1 2 3 4 5))   ; => (3 1 5 2 4) (varies)
```
