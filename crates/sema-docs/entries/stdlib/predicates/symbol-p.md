---
name: "symbol?"
module: "predicates"
section: "Type Predicates"
params: [{ name: x, type: any }]
returns: "bool"
---

Test if a value is a symbol.

```sema
(symbol? 'x)     ; => #t
(symbol? "x")    ; => #f
```
