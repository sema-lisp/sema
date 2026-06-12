---
name: "hash-map"
module: "maps"
section: "Construction"
returns: map
---

Construct a map from alternating key/value arguments. Requires an even number of arguments. Alias of `map/new`.

```sema
(hash-map :a 1 :b 2)   ; => {:a 1 :b 2}
(hash-map)             ; => {}
```
