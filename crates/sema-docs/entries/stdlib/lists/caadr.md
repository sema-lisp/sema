---
name: "caadr"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
---

Equivalent to `(car (car (cdr x)))`.

```sema
(caadr '(1 (2 3) 4))   ; => 2
```
