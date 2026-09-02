---
name: "keys"
module: "maps"
section: "Maps"
params: [{ name: m, type: map }]
returns: "list"
see_also: ["vals", "map/entries", "count", "hashmap/keys"]
---

Return the keys of a map as a list. For an ordered map the keys come back sorted, and `keys` and `vals` line up positionally — `(nth (keys m) i)` is the key for `(nth (vals m) i)`.

```sema
(keys {:a 1 :b 2})        ; => (:a :b)
(keys {:c 3 :a 1 :b 2})   ; => (:a :b :c)  (ordered map -> sorted)
(keys {})                 ; => ()
```
