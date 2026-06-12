---
name: "equal?"
module: "predicates"
section: "Equality"
params: [{ name: a, type: any }, { name: b, type: any }]
returns: bool
---

Test structural (deep) equality of two values.

```sema
(equal? '(1 2) '(1 2))   ; => #t
(equal? "ab" "ab")       ; => #t
(equal? 1 2)             ; => #f
```
