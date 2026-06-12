---
name: "map/deep-merge"
module: "maps"
section: "Nested Map Operations"
---

Recursively merge maps. Nested maps are merged rather than replaced. Non-map values in the overlay override the base.

```sema
(map/deep-merge {:a {:b 1 :c 2}} {:a {:b 99}})      ; => {:a {:b 99 :c 2}}
(map/deep-merge {:a {:b 1}} {:a 42})                 ; => {:a 42}
(map/deep-merge {:a 1} {:b 2} {:c 3})               ; => {:a 1 :b 2 :c 3}
```
