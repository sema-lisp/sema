---
name: "vector?"
module: "predicates"
section: "Collection Predicates"
params: [{ name: x, type: any }]
returns: "bool"
---

Test if a value is a vector.

```sema
(vector? [1])   ; => #t
(vector? '(1))  ; => #f
```
