---
name: "hash-map"
module: "maps"
section: "Construction"
returns: map
syntax: "(hash-map key value ...)"
see_also: ["map/new", "hashmap/new", "map/from-entries"]
---

Construct an ordered map from alternating key/value arguments. Requires an even number of arguments. Alias of `map/new`. (Despite the name, this builds the ordered `:map` type, not the unordered `:hashmap` from `hashmap/new`.)

```sema
(hash-map :a 1 :b 2)   ; => {:a 1 :b 2}
(hash-map)             ; => {}
```
