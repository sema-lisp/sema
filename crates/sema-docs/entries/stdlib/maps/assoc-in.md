---
name: "assoc-in"
module: "maps"
section: "Nested Map Operations"
params: [{ name: m, type: map }, { name: path, type: "list | vector" }, { name: value, type: any }]
returns: map
---

Return a copy of `m` with `value` set at the nested key `path`, creating intermediate maps as needed. Same as `map/assoc-in`.

```sema
(assoc-in {:a {:b 1}} [:a :c] 2)   ; => {:a {:b 1 :c 2}}
(assoc-in {} [:x :y] 9)            ; => {:x {:y 9}}
```
