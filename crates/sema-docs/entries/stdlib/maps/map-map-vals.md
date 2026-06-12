---
name: "map/map-vals"
module: "maps"
section: "Higher-Order Map Operations"
---

Apply a function to every value in a map.

```sema
(map/map-vals (fn (v) (* v 2)) {:a 1 :b 2})   ; => {:a 2 :b 4}
```
