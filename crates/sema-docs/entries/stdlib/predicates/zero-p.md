---
name: "zero?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: n, type: number }]
returns: "bool"
---

Test if a number is zero.

```sema
(zero? 0)   ; => #t
(zero? 1)   ; => #f
```
