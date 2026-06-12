---
name: "get-in"
module: "maps"
section: "Nested Map Operations"
params: [{ name: m, type: map }, { name: path, type: "list | vector" }, { name: default, type: any }]
---

Access a value at a nested key path. Returns `nil` (or `default`) if any key along the path is missing. Same as `map/get-in`.

```sema
(get-in {:a {:b {:c 42}}} [:a :b :c])         ; => 42
(get-in {:a {:b 1}} [:a :c])                   ; => nil
(get-in {:a {:b 1}} [:a :c] "default")         ; => "default"
```
