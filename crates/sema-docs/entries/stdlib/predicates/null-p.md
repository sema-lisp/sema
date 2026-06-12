---
name: "null?"
module: "predicates"
section: "Emptiness Predicates"
---

Test if a value is the empty list or `nil`.

```sema
(null? '())    ;; => #t
(null? nil)    ;; => #t
(null? '(1))   ;; => #f
```
