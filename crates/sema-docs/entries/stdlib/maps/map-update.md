---
name: "map/update"
module: "maps"
section: "Higher-Order Map Operations"
---

Update a value at a key by applying a function.

```sema
(map/update {:a 1} :a (fn (v) (+ v 10)))   ; => {:a 11}
```
