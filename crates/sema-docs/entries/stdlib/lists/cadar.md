---
name: "cadar"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
returns: "any"
see_also: ["car", "cdr", "cadr", "nth"]
---

Equivalent to `(car (cdr (car x)))`.

```sema
(cadar '((1 2 3) 4))   ; => 2
```
