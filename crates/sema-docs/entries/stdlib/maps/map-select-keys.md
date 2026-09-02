---
name: "map/select-keys"
module: "maps"
section: "Higher-Order Map Operations"
params: [{ name: m, type: map }, { name: keys, type: "list | vector" }]
returns: "map"
see_also: ["map/except", "get", "list/pluck"]
---

Return a new map containing only the listed keys (a projection / "pick"). Keys in the list that aren't in the map are silently skipped. The inverse — drop the listed keys and keep the rest — is `map/except`.

```sema
(map/select-keys {:a 1 :b 2 :c 3} '(:a :c))   ; => {:a 1 :c 3}
(map/select-keys {:a 1} '(:a :z))             ; => {:a 1}  (absent :z skipped)
```
