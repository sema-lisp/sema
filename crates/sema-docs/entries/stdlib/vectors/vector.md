---
name: "vector"
module: "vectors"
section: "Construction"
syntax: "(vector x ...)"
returns: "vector"
see_also: ["list->vector", "vector->list", "vector?", "list"]
---

Create a vector from its arguments.

```sema
(vector 1 2 3)       ; => [1 2 3]
(vector)             ; => []
(vector "a" "b")     ; => ["a" "b"]
```
