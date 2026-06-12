---
name: "cdddr"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
---

Equivalent to `(cdr (cdr (cdr x)))`.

```sema
(cdddr '(1 2 3 4 5))   ; => (4 5)
```
