---
name: "list->vector"
module: "vectors"
section: "Conversion"
params: [{ name: seq, type: list }]
returns: "vector"
see_also: ["vector->list", "vector", "list"]
---

Convert a list to a vector.

```sema
(list->vector '(1 2 3))   ; => [1 2 3]
(list->vector '())         ; => []
```
