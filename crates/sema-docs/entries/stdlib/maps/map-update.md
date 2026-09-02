---
name: "map/update"
module: "maps"
section: "Higher-Order Map Operations"
params: [{ name: m, type: map }, { name: key, type: any }, { name: f, type: function }]
returns: "map"
see_also: ["map/update-in", "assoc", "map/map-vals", "get"]
---

Return a new map with the value at `key` replaced by `(f current)`, where `current` is the existing value. This is the read-modify-write shortcut for `(assoc m k (f (get m k)))`.

Gotcha: if the key is **absent**, `f` is still called — with `nil` as `current`. Make `f` tolerate `nil` (or ensure the key exists first), otherwise a numeric `f` like `(fn (v) (+ v 1))` will raise a type error. For nested paths use `map/update-in`.

```sema
(map/update {:a 1} :a (fn (v) (+ v 10)))         ; => {:a 11}
(map/update {:count 5} :count (fn (n) (+ n 1)))  ; => {:count 6}

;; Absent key -> f receives nil; guard against it:
(map/update {} :hits (fn (n) (+ (or n 0) 1)))    ; => {:hits 1}
```
