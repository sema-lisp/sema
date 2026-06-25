---
name: "odd?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: n, type: int }]
returns: "bool"
---

Test if an integer is odd.

```sema
(odd? 3)   ; => #t
(odd? 4)   ; => #f
```
