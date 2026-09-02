---
name: "map/sort-keys"
module: "maps"
section: "HashMaps"
params: [{ name: m, type: "map | hashmap" }]
returns: "map"
see_also: ["keys", "hashmap/to-map", "sort"]
---

Return an ordered map with the same entries sorted by key. Mainly useful to turn an unordered `hashmap/new` table into a stable, printable map (or to canonicalize any map for display/comparison).

```sema
(map/sort-keys (hashmap/new :c 3 :a 1 :b 2))   ; => {:a 1 :b 2 :c 3}
(map/sort-keys {:c 3 :a 1 :b 2})               ; => {:a 1 :b 2 :c 3}
```
