---
name: "first"
module: "vectors"
section: "Indexed Access"
params: [{ name: vec, type: vector, doc: "vector or list; nil if empty" }]
returns: "any"
---

Return the first element of a vector (or list). Returns `nil` for empty vectors.

```sema
(first [1 2 3])   ; => 1
(first [])        ; => nil
```
