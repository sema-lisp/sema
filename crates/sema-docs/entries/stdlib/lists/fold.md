---
name: "fold"
module: "lists"
section: "Reduction"
params: [{ name: f, type: function }, { name: init, type: any }, { name: seq, type: "list | vector" }]
returns: "any"
see_also: ["foldl", "foldr", "reduce"]
---

Left fold: combine elements of `seq` from the left starting with `init`, calling `(f acc elem)` for each. Alias of `foldl`.

```sema
(fold + 0 '(1 2 3 4))   ; => 10
(fold * 1 '(1 2 3 4))   ; => 24
```

See `foldl` for the argument-order details and the contrast with `foldr` (right fold).

