---
name: "mapcar"
module: "lists"
section: "Transformation"
params: [{ name: f, type: function }, { name: seq, type: "list | vector" }]
returns: list
---

Apply `f` to each element of `seq`, returning a list of results. Alias of `map`.

```sema
(mapcar (fn (x) (* x x)) '(1 2 3))   ; => (1 4 9)
```
