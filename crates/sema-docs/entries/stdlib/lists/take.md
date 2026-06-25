---
name: "take"
module: "lists"
section: "Sublists"
params: [{ name: n, type: int }, { name: lst, type: list }]
returns: "list"
---

Take the first N elements.

```sema
(take 3 '(1 2 3 4 5))   ; => (1 2 3)
(take 10 '(1 2))         ; => (1 2)
```
