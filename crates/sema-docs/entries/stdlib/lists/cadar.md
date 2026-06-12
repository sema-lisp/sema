---
name: "cadar"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
---

Equivalent to `(car (cdr (car x)))`.

```sema
(cadar '((1 2 3) 4))   ; => 2
```
