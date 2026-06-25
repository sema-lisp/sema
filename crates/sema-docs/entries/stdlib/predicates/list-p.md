---
name: "list?"
module: "predicates"
section: "Collection Predicates"
params: [{ name: x, type: any }]
returns: "bool"
---

Test if a value is a list.

```sema
(list? '(1))    ; => #t
(list? 42)      ; => #f
```
