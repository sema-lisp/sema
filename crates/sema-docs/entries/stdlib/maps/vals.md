---
name: "vals"
module: "maps"
section: "Maps"
params: [{ name: m, type: "map | hashmap" }]
returns: "list"
see_also: ["keys", "map/entries", "count"]
---

Return the values of a map as a list, in the same order as `keys` returns the corresponding keys (sorted by key for an ordered map).

```sema
(vals {:a 1 :b 2})        ; => (1 2)
(vals {:c 3 :a 1 :b 2})   ; => (1 2 3)  (ordered by key)
(vals {})                 ; => ()
```
