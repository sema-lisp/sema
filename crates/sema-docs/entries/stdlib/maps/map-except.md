---
name: "map/except"
module: "maps"
section: "HashMaps"
---

Remove specified keys from a map (inverse of `map/select-keys`).

```sema
(map/except {:a 1 :b 2 :c 3} '(:b))       ; => {:a 1 :c 3}
(map/except {:a 1 :b 2 :c 3} '(:a :c))    ; => {:b 2}
```
