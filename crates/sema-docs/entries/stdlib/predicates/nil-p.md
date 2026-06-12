---
name: "nil?"
module: "predicates"
section: "Emptiness Predicates"
---

Test if a value is `nil` specifically (not the empty list).

```sema
(nil? nil)     ;; => #t
(nil? '())     ;; => #f
(nil? 0)       ;; => #f
```
