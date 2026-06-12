---
name: "cdaar"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
---

Equivalent to `(cdr (car (car x)))`.

```sema
(cdaar '(((1 2) 3) 4))   ; => (2)
```
