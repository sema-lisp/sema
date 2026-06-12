---
name: "eq?"
module: "predicates"
section: "Equality"
---

Test structural equality. `equal?` is an alias.

```sema
(eq? 'a 'a)           ; => #t
(eq? '(1 2) '(1 2))   ; => #t
(eq? 1 2)             ; => #f
```
