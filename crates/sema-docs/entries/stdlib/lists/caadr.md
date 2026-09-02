---
name: "caadr"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
returns: "any"
see_also: ["car", "cdr", "cadr", "nth"]
---

Equivalent to `(car (car (cdr x)))`.

```sema
(caadr '(1 (2 3) 4))   ; => 2
```
