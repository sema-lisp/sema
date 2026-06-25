---
name: "keys"
module: "maps"
section: "Maps"
params: [{ name: m, type: map }]
returns: "list"
---

Return the keys of a map as a list.

```sema
(keys {:a 1 :b 2})   ; => (:a :b)
```
