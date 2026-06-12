---
name: "map/get-in"
module: "maps"
section: "Nested Map Operations"
---

Access a value at a nested key path. Returns `nil` (or a default) if any key is missing.

```sema
(map/get-in {:a {:b {:c 42}}} [:a :b :c])           ; => 42
(map/get-in {:a {:b 1}} [:a :c])                     ; => nil
(map/get-in {:a {:b 1}} [:a :c] "default")           ; => "default"
```
