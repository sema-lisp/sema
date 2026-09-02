---
name: "map/map-vals"
module: "maps"
section: "Higher-Order Map Operations"
params: [{ name: f, type: function }, { name: m, type: map }]
returns: "map"
see_also: ["map/map-keys", "map/filter", "map/update", "vals"]
---

Return a new map with `f` applied to every value; keys are left unchanged. The companion `map/map-keys` transforms keys instead. The function receives only the value (not the key) — use `map/filter` if you need to drop entries based on key and value.

```sema
(map/map-vals (fn (v) (* v 2)) {:a 1 :b 2})       ; => {:a 2 :b 4}
(map/map-vals string/upper {:name "ann" :city "oslo"})
; => {:city "OSLO" :name "ANN"}
```
