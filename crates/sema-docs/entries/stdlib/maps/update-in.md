---
name: "update-in"
module: "maps"
section: "Nested Map Operations"
params: [{ name: m, type: map }, { name: path, type: "list | vector" }, { name: f, type: function }]
returns: map
---

Return a copy of `m` with the value at the nested key `path` replaced by `(f current)`, where `current` is the existing value (`nil` if missing). Same as `map/update-in`.

```sema
(update-in {:a {:b 1}} [:a :b] (fn (x) (+ x 10)))   ; => {:a {:b 11}}
```
