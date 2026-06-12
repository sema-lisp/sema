---
name: "fold"
module: "lists"
section: "Reduction"
params: [{ name: f, type: function }, { name: init, type: any }, { name: seq, type: "list | vector" }]
---

Left fold: combine elements of `seq` from the left starting with `init`, calling `(f acc elem)` for each. Alias of `foldl`.

```sema
(fold + 0 '(1 2 3 4))   ; => 10
```
