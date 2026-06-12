---
name: "map/update-in"
module: "maps"
section: "Nested Map Operations"
---

Update a value at a nested key path by applying a function.

```sema
(map/update-in {:a {:b 10}} [:a :b] #(+ % 1))       ; => {:a {:b 11}}
```
