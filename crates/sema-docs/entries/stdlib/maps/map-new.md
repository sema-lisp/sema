---
name: "map/new"
module: "maps"
section: "Maps"
syntax: "(map/new key val ...)"
returns: "map"
see_also: ["hash-map", "hashmap/new", "map/from-entries"]
---

Create an ordered map from alternating key/value arguments — the function form of the `{...}` literal. Requires an even number of arguments. `hash-map` is an alias; for the unordered hash-table type use `hashmap/new`.

```sema
(map/new :a 1 :b 2)   ; => {:a 1 :b 2}
(map/new)             ; => {}
```
