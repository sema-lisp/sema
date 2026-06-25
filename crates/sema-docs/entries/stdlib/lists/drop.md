---
name: "drop"
module: "lists"
section: "Sublists"
params: [{ name: n, type: int }, { name: lst, type: list }]
returns: "list"
---

Drop the first N elements.

```sema
(drop 2 '(1 2 3 4 5))   ; => (3 4 5)
```
