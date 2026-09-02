---
name: "map/except"
module: "maps"
section: "HashMaps"
params: [{ name: m, type: map }, { name: keys, type: "list | vector" }]
returns: "map"
see_also: ["map/select-keys", "dissoc", "map/filter"]
---

Remove specified keys from a map (inverse of `map/select-keys`).

```sema
(map/except {:a 1 :b 2 :c 3} '(:b))       ; => {:a 1 :c 3}
(map/except {:a 1 :b 2 :c 3} '(:a :c))    ; => {:b 2}
```
