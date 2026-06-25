---
name: "get"
module: "maps"
section: "Maps"
params: [{ name: m, type: map }, { name: key, type: any }, { name: default, type: any, doc: "optional value returned when key is absent (defaults to nil)" }]
returns: "any"
---

Look up a value by key. Works on both maps and hashmaps.

```sema
(get {:a 1 :b 2} :a)   ; => 1
(get {:a 1 :b 2} :z)   ; => nil
```
