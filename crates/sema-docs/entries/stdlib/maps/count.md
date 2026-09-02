---
name: "count"
module: "maps"
section: "Maps"
params: [{ name: coll, type: any, doc: "list, vector, map, or string" }]
returns: "int"
see_also: ["length", "keys", "empty?"]
---

Return the number of elements in a collection. For a map that's the number of key-value pairs; `count` also works on lists, vectors, and strings (where it counts characters).

```sema
(count {:a 1 :b 2})   ; => 2
(count {})            ; => 0
(count '(1 2 3))      ; => 3
(count [1 2 3])       ; => 3
(count "hi")          ; => 2
```
