---
name: "map/map-keys"
module: "maps"
section: "Higher-Order Map Operations"
params: [{ name: f, type: function }, { name: m, type: map }]
returns: "map"
see_also: ["map/map-vals", "map/filter", "keys"]
---

Return a new map with `f` applied to every key; values are left unchanged. The companion `map/map-vals` transforms values instead. If `f` maps two distinct keys onto the same result, the later entry wins (keys collapse).

```sema
(map/map-keys
  (fn (k) (string/to-keyword (string/upper (keyword/to-string k))))
  {:a 1})
; => {:A 1}
```
