---
name: "hash-map?"
module: "maps"
section: "Predicates"
params: [{ name: x, type: any }]
returns: bool
---

Return `#t` if `x` is a map. Alias of `map?`.

```sema
(hash-map? {:a 1})   ; => #t
(hash-map? '(1 2))   ; => #f
```
