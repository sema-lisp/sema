---
name: "rest"
module: "vectors"
section: "Indexed Access"
params: [{ name: vec, type: vector }]
returns: "vector"
---

Return everything after the first element. **Preserves type** — vector in, vector out.

```sema
(rest [1 2 3])    ; => [2 3]
(rest [])         ; => []
(rest [1])        ; => []
```
