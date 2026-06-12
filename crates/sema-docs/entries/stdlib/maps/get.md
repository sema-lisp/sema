---
name: "get"
module: "maps"
section: "Maps"
---

Look up a value by key. Works on both maps and hashmaps.

```sema
(get {:a 1 :b 2} :a)   ; => 1
(get {:a 1 :b 2} :z)   ; => nil
```
