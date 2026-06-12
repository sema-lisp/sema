---
name: "map/filter"
module: "maps"
section: "Higher-Order Map Operations"
---

Filter entries by a predicate that takes key and value.

```sema
(map/filter (fn (k v) (> v 1)) {:a 1 :b 2 :c 3})   ; => {:b 2 :c 3}
```
