---
name: "list/take-last"
module: "lists"
section: "Slicing"
params: [{ name: n, type: int }, { name: seq, type: list }]
returns: "list"
see_also: ["list/drop-last", "take", "last"]
---

Return the last `n` elements (the tail counterpart to `take`). Clamps to the sequence length, so asking for more than exists returns the whole sequence.

```sema
(list/take-last 2 (list 1 2 3 4))   ; => (3 4)
(list/take-last 9 (list 1 2))       ; => (1 2)
```
