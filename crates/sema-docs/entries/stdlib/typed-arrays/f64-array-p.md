---
name: "f64-array?"
module: "typed-arrays"
section: "Type Predicates"
aliases: ["i64-array?"]
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["f64-array", "i64-array", "vector?", "type-of"]
---

Test whether a value is a typed array.

```sema
(f64-array? (f64-array 1.0 2.0))  ; => #t
(f64-array? '(1.0 2.0))           ; => #f
(i64-array? (i64-array 1 2))      ; => #t
```
