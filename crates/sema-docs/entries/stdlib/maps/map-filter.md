---
name: "map/filter"
module: "maps"
section: "Higher-Order Map Operations"
params: [{ name: pred, type: function }, { name: m, type: map }]
returns: "map"
see_also: ["map/map-vals", "map/map-keys", "filter", "map/select-keys"]
---

Keep only the entries for which `(pred k v)` is truthy, returning a new map. The predicate receives **both** the key and the value (two arguments), so you can filter on either. To transform values instead of dropping entries, use `map/map-vals`; to keep a fixed set of keys, use `map/select-keys`.

```sema
(map/filter (fn (k v) (> v 1)) {:a 1 :b 2 :c 3})       ; => {:b 2 :c 3}
(map/filter (fn (k v) (even? v)) {:a 1 :b 2 :c 3 :d 4}) ; => {:b 2 :d 4}
```
