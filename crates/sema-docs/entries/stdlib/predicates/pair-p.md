---
name: "pair?"
module: "predicates"
section: "Collection Predicates"
params: [{ name: x, type: any }]
returns: "bool"
---

Test if a value is a non-empty list (Scheme compatibility).

```sema
(pair? '(1 2))   ; => #t
(pair? '())      ; => #f
```
