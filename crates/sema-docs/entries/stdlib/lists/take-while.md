---
name: "take-while"
module: "lists"
section: "Slicing"
params: [{ name: pred, type: function }, { name: seq, type: "list | vector" }]
returns: list
---

Return the leading elements of `seq` for which `pred` is truthy, stopping at the first element that fails. Same as `list/take-while`.

```sema
(take-while (fn (x) (< x 4)) '(1 2 3 4 1))   ; => (1 2 3)
```
