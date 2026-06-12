---
name: "caar"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
---

Equivalent to `(car (car x))`.

```sema
(caar '((1 2) 3))   ; => 1
```
