---
name: "caaar"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
---

Equivalent to `(car (car (car x)))`.

```sema
(caaar '(((1 2) 3) 4))   ; => 1
```
