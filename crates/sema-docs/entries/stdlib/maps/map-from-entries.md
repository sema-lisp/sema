---
name: "map/from-entries"
module: "maps"
section: "Maps"
params: [{ name: entries, type: "list | vector" }]
returns: "map"
see_also: ["map/entries", "map/zip", "hash-map"]
---

Build a map from a sequence of `(key value)` pairs — the inverse of `map/entries`. Each pair may be a list or a vector. On duplicate keys, the last pair wins.

```sema
(map/from-entries '((:a 1) (:b 2)))   ; => {:a 1 :b 2}
(map/from-entries '([:a 1] [:b 2]))   ; => {:a 1 :b 2}  (vector pairs ok)
(map/from-entries (map/entries {:x 9}))   ; => {:x 9}   (roundtrip)
```
