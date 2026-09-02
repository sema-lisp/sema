---
name: "caaar"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
returns: "any"
see_also: ["car", "cdr", "cadr", "nth"]
---

Equivalent to `(car (car (car x)))`.

```sema
(caaar '(((1 2) 3) 4))   ; => 1
```
