---
name: "eq?"
module: "predicates"
section: "Equality"
params: [{ name: a, type: any }, { name: b, type: any }]
returns: "bool"
---

Test structural equality. `equal?` is an alias.

```sema
(eq? 'a 'a)           ; => #t
(eq? '(1 2) '(1 2))   ; => #t
(eq? 1 2)             ; => #f
```
