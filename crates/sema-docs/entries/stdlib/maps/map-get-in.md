---
name: "map/get-in"
module: "maps"
section: "Nested Map Operations"
params: [{ name: m, type: map }, { name: path, type: "list | vector" }, { name: default, type: any, doc: "optional; defaults to nil" }]
returns: "any"
see_also: ["get-in", "get", "map/assoc-in", "map/update-in"]
---

Access a value at a nested key path. Returns `nil` (or a default) if any key is missing.

```sema
(map/get-in {:a {:b {:c 42}}} [:a :b :c])           ; => 42
(map/get-in {:a {:b 1}} [:a :c])                     ; => nil
(map/get-in {:a {:b 1}} [:a :c] "default")           ; => "default"
```
