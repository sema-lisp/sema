---
name: "cddar"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
---

Equivalent to `(cdr (cdr (car x)))`.

```sema
(cddar '((1 2 3) 4))   ; => (3)
```
