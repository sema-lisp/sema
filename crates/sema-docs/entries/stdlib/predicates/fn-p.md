---
name: "fn?"
module: "predicates"
section: "Type Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["procedure?", "type-of", "apply"]
---

Test if a value is a function. `procedure?` is an alias.

```sema
(fn? car)        ; => #t
(fn? 42)         ; => #f
```
