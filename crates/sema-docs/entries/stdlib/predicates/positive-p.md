---
name: "positive?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: n, type: number }]
returns: "bool"
---

Test if a number is positive.

```sema
(positive? 1)    ; => #t
(positive? -1)   ; => #f
```
