---
name: "cddr"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: list }]
returns: "any"
see_also: ["car", "cdr", "cadr", "nth"]
---

Equivalent to `(cdr (cdr x))`.

```sema
(cddr '(1 2 3 4))   ; => (3 4)
```
