---
name: "map?"
module: "predicates"
section: "Collection Predicates"
params: [{ name: value, type: any }]
returns: "bool"
see_also: ["hash-map?", "list?", "contains?", "type-of"]
---

Test if a value is a map.

```sema
(map? {:a 1})   ; => #t
(map? '())      ; => #f
```
