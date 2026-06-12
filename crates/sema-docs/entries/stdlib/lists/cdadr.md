---
name: "cdadr"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
---

Equivalent to `(cdr (car (cdr x)))`.

```sema
(cdadr '(1 (2 3) 4))   ; => (3)
```
