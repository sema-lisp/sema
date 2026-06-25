---
name: "foldr"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: f, type: function, doc: "called as (f elem acc)" }, { name: init, type: any }, { name: list, type: list }]
returns: "any"
---

Right fold. `(foldr f init list)` — accumulates from right to left.

```sema
(foldr cons '() '(1 2 3))   ; => (1 2 3)
```
