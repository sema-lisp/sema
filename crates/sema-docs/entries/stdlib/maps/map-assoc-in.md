---
name: "map/assoc-in"
module: "maps"
section: "Nested Map Operations"
---

Set a value at a nested key path. Creates intermediate maps if they don't exist.

```sema
(map/assoc-in {:a {:b 1}} [:a :b] 42)                ; => {:a {:b 42}}
(map/assoc-in {} [:a :b :c] 99)                      ; => {:a {:b {:c 99}}}
```
