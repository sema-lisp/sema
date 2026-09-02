---
name: "map/update-in"
module: "maps"
section: "Nested Map Operations"
params: [{ name: m, type: map }, { name: path, type: "list | vector" }, { name: f, type: function }]
returns: "map"
see_also: ["update-in", "map/update", "map/assoc-in", "map/get-in"]
---

Update a value deep inside nested maps by applying a function at the given key path. Returns a new map; the original is untouched. The function receives the existing value (`nil` if the path is missing) — for a plain replacement use `map/assoc-in` instead.

```sema
(map/update-in {:a {:b 10}} [:a :b] #(+ % 1))       ; => {:a {:b 11}}
(map/update-in {:user {:stats {:visits 3}}}
               [:user :stats :visits]
               (fn (n) (+ n 1)))
; => {:user {:stats {:visits 4}}}
```
