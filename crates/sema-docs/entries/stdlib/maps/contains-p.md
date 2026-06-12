---
name: "contains?"
module: "maps"
section: "Maps"
---

Test if a map contains a key.

```sema
(contains? {:a 1} :a)   ; => #t
(contains? {:a 1} :b)   ; => #f
```
