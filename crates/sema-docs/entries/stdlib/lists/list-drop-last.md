---
name: "list/drop-last"
module: "lists"
section: "Slicing"
params: [{ name: n, type: int }, { name: seq, type: list }]
returns: "list"
see_also: ["list/take-last", "drop", "list/drop-while"]
---

Return all but the last `n` elements (drops from the tail; the counterpart to `drop`). Clamps to empty.

```sema
(list/drop-last 2 (list 1 2 3 4))   ; => (1 2)
(list/drop-last 9 (list 1 2))       ; => ()
```
