---
name: "vector?"
module: "vectors"
section: "Predicates & Introspection"
params: [{ name: x, type: any }]
returns: "bool"
---

Test whether a value is a vector.

```sema
(vector? [1 2 3])          ; => #t
(vector? '(1 2 3))         ; => #f
(vector? 42)               ; => #f
```
