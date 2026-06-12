---
name: "boolean?"
module: "predicates"
section: "Type Predicates"
params: [{ name: x, type: any }]
returns: bool
---

Return `#t` if `x` is a boolean (`#t` or `#f`). Alias of `bool?`.

```sema
(boolean? #t)    ; => #t
(boolean? 0)     ; => #f
```
