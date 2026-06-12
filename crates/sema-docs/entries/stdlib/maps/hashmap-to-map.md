---
name: "hashmap/to-map"
module: "maps"
section: "HashMaps"
---

Convert a hashmap to a sorted map.

```sema
(hashmap/to-map (hashmap/new :b 2 :a 1))   ; => {:a 1 :b 2}
```
