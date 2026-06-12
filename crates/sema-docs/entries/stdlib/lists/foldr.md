---
name: "foldr"
module: "lists"
section: "Higher-Order Functions"
---

Right fold. `(foldr f init list)` — accumulates from right to left.

```sema
(foldr cons '() '(1 2 3))   ; => (1 2 3)
```
