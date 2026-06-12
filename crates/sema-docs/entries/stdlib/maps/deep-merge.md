---
name: "deep-merge"
module: "maps"
section: "Combining"
returns: map
---

Recursively merge maps left to right. Nested maps are merged; non-map values from later arguments overwrite earlier ones. Same as `map/deep-merge`.

```sema
(deep-merge {:a {:x 1}} {:a {:y 2}})   ; => {:a {:x 1 :y 2}}
(deep-merge {:a 1} {:a 2})             ; => {:a 2}
```
