---
name: "map?"
module: "predicates"
section: "Collection Predicates"
params: [{ name: value, type: any }]
returns: "bool"
---

Test if a value is a map.

```sema
(map? {:a 1})   ; => #t
(map? '())      ; => #f
```
