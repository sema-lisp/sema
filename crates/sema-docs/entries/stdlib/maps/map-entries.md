---
name: "map/entries"
module: "maps"
section: "Maps"
params: [{ name: m, type: map }]
returns: "list"
---

Return the entries as a list of key-value pairs.

```sema
(map/entries {:a 1 :b 2})   ; => ((:a 1) (:b 2))
```
