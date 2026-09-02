---
name: "hashmap/to-map"
module: "maps"
section: "HashMaps"
params: [{ name: hm, type: hashmap }]
returns: "map"
see_also: ["hashmap/new", "map/sort-keys", "hash-map"]
---

Convert a hashmap into an ordered map (keys sorted). Use this to get a stable, printable, comparable value out of the unordered `:hashmap` type.

```sema
(hashmap/to-map (hashmap/new :b 2 :a 1))   ; => {:a 1 :b 2}
```
