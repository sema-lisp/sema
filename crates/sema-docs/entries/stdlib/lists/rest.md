---
name: "rest"
module: "lists"
section: "Construction & Access"
params: [{ name: lst, type: list }]
returns: "list"
---

Alias for `cdr`. Return everything after the first element. **Preserves type** — list in, list out; vector in, vector out.

```sema
(rest '(1 2 3))    ; => (2 3)
(rest [1 2 3])     ; => [2 3]
(rest [])          ; => []
(rest [1])         ; => []
```
