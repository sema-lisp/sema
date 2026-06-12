---
name: "procedure?"
module: "predicates"
section: "Type Predicates"
params: [{ name: x, type: any }]
returns: bool
---

Return `#t` if `x` is callable (a lambda or native function). Alias of `fn?`.

```sema
(procedure? car)        ; => #t
(procedure? (fn (x) x)) ; => #t
(procedure? 42)         ; => #f
```
