---
name: "even?"
module: "math"
section: "Numeric Predicates"
params: [{ name: n, type: int }]
returns: "bool"
---

Test if an integer is even.

```sema
(even? 4)      ; => #t
(even? 3)      ; => #f
```
