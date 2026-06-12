---
name: "map/map-keys"
module: "maps"
section: "Higher-Order Map Operations"
---

Apply a function to every key in a map.

```sema
(map/map-keys
  (fn (k) (string/to-keyword (string/upper (keyword/to-string k))))
  {:a 1})
; => {:A 1}
```
