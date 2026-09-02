---
name: "vector->list"
module: "vectors"
section: "Conversion"
params: [{ name: v, type: vector }]
returns: "list"
see_also: ["list->vector", "vector", "list"]
---

Convert a vector to a list.

```sema
(vector->list [1 2 3])   ; => (1 2 3)
(vector->list [])         ; => ()
```
