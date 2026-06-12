---
name: "drop-while"
module: "lists"
section: "Slicing"
params: [{ name: pred, type: function }, { name: seq, type: "list | vector" }]
returns: list
---

Drop the leading elements of `seq` for which `pred` is truthy, returning the rest unchanged. Same as `list/drop-while`.

```sema
(drop-while (fn (x) (< x 4)) '(1 2 3 4 1))   ; => (4 1)
```
