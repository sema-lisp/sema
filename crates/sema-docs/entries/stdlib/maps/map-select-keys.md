---
name: "map/select-keys"
module: "maps"
section: "Higher-Order Map Operations"
---

Select only the given keys from a map.

```sema
(map/select-keys {:a 1 :b 2 :c 3} '(:a :c))   ; => {:a 1 :c 3}
```
