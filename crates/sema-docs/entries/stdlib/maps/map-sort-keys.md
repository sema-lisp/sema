---
name: "map/sort-keys"
module: "maps"
section: "HashMaps"
---

Sort a map by its keys. Converts hashmaps to sorted maps.

```sema
(map/sort-keys (hashmap/new :c 3 :a 1 :b 2))   ; => {:a 1 :b 2 :c 3}
```
