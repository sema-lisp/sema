---
name: "caar"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
returns: "any"
see_also: ["car", "cdr", "cadr", "nth"]
---

Equivalent to `(car (car x))`.

```sema
(caar '((1 2) 3))   ; => 1
```
