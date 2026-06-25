---
name: "count"
module: "maps"
section: "Maps"
params: [{ name: coll, type: any, doc: "list, vector, map, or string" }]
returns: "int"
---

Return the number of key-value pairs.

```sema
(count {:a 1 :b 2})   ; => 2
```
