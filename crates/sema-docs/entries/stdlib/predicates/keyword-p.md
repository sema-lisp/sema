---
name: "keyword?"
module: "predicates"
section: "Type Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["symbol?", "string?", "type-of"]
---

Test if a value is a keyword.

```sema
(keyword? :k)    ; => #t
(keyword? "k")   ; => #f
```
