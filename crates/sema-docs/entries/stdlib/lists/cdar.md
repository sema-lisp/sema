---
name: "cdar"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
---

Equivalent to `(cdr (car x))`.

```sema
(cdar '((1 2 3) 4))   ; => (2 3)
```
