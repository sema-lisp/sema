---
name: "filter"
module: "lists"
section: "Higher-Order Functions"
---

Return elements that satisfy a predicate.

```sema
(filter even? '(1 2 3 4 5))   ; => (2 4)
(filter string? '(1 "a" 2))   ; => ("a")
```
