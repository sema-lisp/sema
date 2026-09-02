---
name: "cadr"
module: "lists"
section: "Construction & Access"
aliases: ["caddr"]
params: [{ name: lst, type: list }]
returns: "any"
see_also: ["car", "cdr", "cddr", "nth"]
---

Compositions of `car` and `cdr`. Available: `caar`, `cadr`, `cdar`, `cddr`, `caaar`, `caadr`, `cadar`, `caddr`, `cdaar`, `cdadr`, `cddar`, `cdddr`.

```sema
(cadr '(1 2 3))    ; => 2
(caddr '(1 2 3))   ; => 3
```
