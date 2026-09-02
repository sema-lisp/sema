---
name: "map/entries"
module: "maps"
section: "Maps"
params: [{ name: m, type: map }]
returns: "list"
see_also: ["keys", "vals", "map/from-entries"]
---

Return the entries as a list of `(key value)` pairs — handy for iterating a map with list functions like `map`, `filter`, or `foldl`. `map/from-entries` is the inverse and rebuilds a map from such a list.

```sema
(map/entries {:a 1 :b 2})   ; => ((:a 1) (:b 2))
(map/entries {})            ; => ()
```
