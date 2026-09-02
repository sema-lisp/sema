---
name: "bool?"
module: "predicates"
section: "Type Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["boolean?", "not", "type-of"]
---

Test if a value is a boolean. `boolean?` is an alias.

```sema
(bool? #t)   ; => #t
(bool? 0)    ; => #f
```
