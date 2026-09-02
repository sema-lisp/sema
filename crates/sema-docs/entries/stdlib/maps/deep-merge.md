---
name: "deep-merge"
module: "maps"
section: "Combining"
returns: map
syntax: "(deep-merge map ...)"
see_also: ["merge", "map/deep-merge", "assoc-in"]
---

Recursively merge maps left to right. Nested maps are merged; non-map values from later arguments overwrite earlier ones. Same as `map/deep-merge`. The result is always an ordered map, regardless of the input map types (unlike `merge`, which preserves the type of its first argument).

```sema
(deep-merge {:a {:x 1}} {:a {:y 2}})   ; => {:a {:x 1 :y 2}}
(deep-merge {:a 1} {:a 2})             ; => {:a 2}
```
